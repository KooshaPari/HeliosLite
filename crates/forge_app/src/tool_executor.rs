use std::path::PathBuf;
use std::sync::Arc;
use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectIntent {
    pub effect_id: String,
    pub durable_effort_ref: String,
    pub worker_attempt_id: String,
    pub tool_name: String,
    pub target_fingerprint: String,
}

#[async_trait]
pub trait EffectRecoveryAdapter: Send + Sync {
    async fn begin(&self, intent: EffectIntent) -> anyhow::Result<()>;
    async fn mark_dispatched(&self, effect_id: &str) -> anyhow::Result<()>;
    async fn confirm_success(&self, effect_id: &str) -> anyhow::Result<()>;
    async fn mark_uncertain(&self, effect_id: &str, reason: &str) -> anyhow::Result<()>;
}

#[derive(Clone)]
pub struct EffectRecoveryContext {
    pub durable_effort_ref: String,
    pub worker_attempt_id: String,
    pub adapter: Arc<dyn EffectRecoveryAdapter>,
}

async fn execute_effect<T, F, Fut>(
    recovery: Option<&EffectRecoveryContext>,
    intent: Option<EffectIntent>,
    operation: F,
) -> anyhow::Result<T>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<T>>,
{
    let active = match (recovery, intent) {
        (Some(recovery), Some(intent)) => {
            let id = intent.effect_id.clone();
            recovery.adapter.begin(intent).await?;
            recovery.adapter.mark_dispatched(&id).await?;
            Some((id, Arc::clone(&recovery.adapter)))
        }
        _ => None,
    };

    let result = operation().await;
    match result {
        Ok(value) => {
            if let Some((effect_id, adapter)) = active.as_ref()
                && let Err(confirm_error) = adapter.confirm_success(effect_id).await
            {
                let reason = format!(
                    "tool side effect completed but durable confirmation failed: {confirm_error}"
                );
                let _ = adapter.mark_uncertain(effect_id, &reason).await;
                return Err(anyhow!(reason));
            }
            Ok(value)
        }
        Err(error) => {
            if let Some((effect_id, adapter)) = active.as_ref() {
                let reason = format!("tool execution returned error after dispatch: {error}");
                let _ = adapter.mark_uncertain(effect_id, &reason).await;
            }
            Err(error)
        }
    }
}

use anyhow::anyhow;
use forge_domain::{CodebaseQueryResult, ToolCallContext, ToolCatalog, ToolOutput};
use forge_sandbox::SandboxConfig;

use crate::fmt::content::FormatContent;
use crate::operation::{TempContentFiles, ToolOperation};
use crate::services::{Services, ShellService};
use crate::{
    AgentRegistry, ConversationService, EnvironmentInfra, FollowUpService, FsPatchService,
    FsReadService, FsRemoveService, FsSearchService, FsUndoService, FsWriteService,
    ImageReadService, NetFetchService, PlanCreateService, ProviderService, SkillFetchService,
    WorkspaceService,
};

pub struct ToolExecutor<S> {
    services: Arc<S>,
    /// Optional OS-level sandbox policy. When `Some`, shell and fetch tool
    /// calls are routed through `forge_sandbox::Sandbox` instead of the bare
    /// shell. Off by default; enable via `config.sandbox`.
    sandbox_policy: Option<SandboxConfig>,
    effect_recovery: Option<EffectRecoveryContext>,
}

impl<
    S: FsReadService
        + ImageReadService
        + FsWriteService
        + FsSearchService
        + WorkspaceService
        + NetFetchService
        + FsRemoveService
        + FsPatchService
        + FsUndoService
        + ShellService
        + FollowUpService
        + ConversationService
        + EnvironmentInfra<Config = forge_config::ForgeConfig>
        + PlanCreateService
        + SkillFetchService
        + AgentRegistry
        + ProviderService
        + Services,
> ToolExecutor<S>
{
    pub fn new(services: Arc<S>) -> Self {
        Self { services, sandbox_policy: None, effect_recovery: None }
    }

    /// Construct with an OS-level sandbox policy. Shell and fetch calls
    /// route through the sandbox.
    pub fn with_sandbox(services: Arc<S>, policy: SandboxConfig) -> Self {
        Self { services, sandbox_policy: Some(policy), effect_recovery: None }
    }

    #[cfg(test)]
    fn with_effect_recovery(mut self, context: EffectRecoveryContext) -> Self {
        self.effect_recovery = Some(context);
        self
    }

    fn require_prior_read(
        &self,
        context: &ToolCallContext,
        raw_path: &str,
        action: &str,
    ) -> anyhow::Result<()> {
        let target_path = self.normalize_path(raw_path.to_string());
        let has_read = context.with_metrics(|metrics| {
            metrics.files_accessed.contains(&target_path)
                || metrics.files_accessed.contains(raw_path)
        })?;

        if has_read {
            Ok(())
        } else {
            Err(anyhow!(
                "You must read the file with the read tool before attempting to {action}.",
                action = action
            ))
        }
    }

    async fn dump_operation(&self, operation: &ToolOperation) -> anyhow::Result<TempContentFiles> {
        match operation {
            ToolOperation::NetFetch { input: _, output } => {
                let config = self.services.get_config()?;
                let original_length = output.content.len();
                let is_truncated = original_length > config.max_fetch_chars;
                let mut files = TempContentFiles::default();

                if is_truncated {
                    files = files.stdout(
                        self.create_temp_file("forge_fetch_", ".txt", &output.content)
                            .await?,
                    );
                }

                Ok(files)
            }
            ToolOperation::Shell { output } => {
                let config = self.services.get_config()?;
                let stdout_lines = output.output.stdout.lines().count();
                let stderr_lines = output.output.stderr.lines().count();
                let stdout_truncated =
                    stdout_lines > config.max_stdout_prefix_lines + config.max_stdout_suffix_lines;
                let stderr_truncated =
                    stderr_lines > config.max_stdout_prefix_lines + config.max_stdout_suffix_lines;

                let mut files = TempContentFiles::default();

                if stdout_truncated {
                    files = files.stdout(
                        self.create_temp_file("forge_shell_stdout_", ".txt", &output.output.stdout)
                            .await?,
                    );
                }
                if stderr_truncated {
                    files = files.stderr(
                        self.create_temp_file("forge_shell_stderr_", ".txt", &output.output.stderr)
                            .await?,
                    );
                }

                Ok(files)
            }
            _ => Ok(TempContentFiles::default()),
        }
    }

    /// Converts a path to absolute by joining it with the current working
    /// directory if it's relative
    fn normalize_path(&self, path: String) -> String {
        let env = self.services.get_environment();
        let path_buf = PathBuf::from(&path);

        if path_buf.is_absolute() {
            path
        } else {
            PathBuf::from(&env.cwd).join(path_buf).display().to_string()
        }
    }

    async fn create_temp_file(
        &self,
        prefix: &str,
        ext: &str,
        content: &str,
    ) -> anyhow::Result<std::path::PathBuf> {
        let path = tempfile::Builder::new()
            .disable_cleanup(true)
            .prefix(prefix)
            .suffix(ext)
            .tempfile()?
            .into_temp_path()
            .to_path_buf();
        self.services
            .write(
                path.to_string_lossy().to_string(),
                content.to_string(),
                true,
            )
            .await?;
        Ok(path)
    }

    /// P1.1: route a shell command through the configured OS-level sandbox.
    /// The shell command string is parsed into program + args via a simple
    /// shlex-style splitter, then passed to `forge_sandbox::Sandbox::run`
    /// against the configured `SandboxConfig`. stdout/stderr/exit_code are
    /// mapped onto `ShellOutput` so the rest of the pipeline is unaffected.
    async fn execute_shell_sandboxed(
        &self,
        command: String,
        cwd: PathBuf,
        env_vars: std::collections::HashMap<String, String>,
        keep_ansi: bool,
        policy: &SandboxConfig,
    ) -> anyhow::Result<crate::services::ShellOutput> {
        use forge_sandbox::Sandbox;

        // Parse the command string into argv. We use a minimal whitespace
        // + quote-aware splitter rather than a shlex dep — the sandbox
        // gets a clean argv instead of a `sh -c` blob.
        let (program, args) = parse_shell_command(&command);

        let mut cfg = policy.clone();
        cfg.command = program;
        cfg.args = args;
        cfg.working_dir = cwd;
        cfg.env = env_vars.into_iter().collect();

        let sandbox = Sandbox::for_platform();
        // The Sandbox::run is async; offload to blocking pool so callers
        // don't need to be inside an explicit runtime.
        let output = tokio::task::spawn_blocking(move || {
            tokio::runtime::Handle::current().block_on(async move { sandbox.run(&cfg).await })
        })
        .await
        .map_err(|e| anyhow!("sandbox task join error: {e}"))?
        .map_err(|e| anyhow!("sandbox execution failed: {e}"))?;

        // Map SandboxOutput -> ShellOutput. `keep_ansi` matches the
        // existing shell service: whether ANSI codes are preserved.
        let _ = keep_ansi;
        let command_str = format!(
            "{}{}{}",
            output.stdout,
            if output.stderr.is_empty() {
                String::new()
            } else {
                format!("\n{}", output.stderr)
            },
            if output.exit_code != 0 {
                format!("\n[exit {}]", output.exit_code)
            } else {
                String::new()
            }
        );
        Ok(crate::services::ShellOutput {
            output: forge_domain::CommandOutput {
                command: command.clone(),
                stdout: output.stdout,
                stderr: output.stderr,
                exit_code: Some(output.exit_code),
            },
            shell: command_str,
            description: None,
        })
    }

    async fn call_internal(
        &self,
        input: ToolCatalog,
        context: &ToolCallContext,
    ) -> anyhow::Result<ToolOperation> {
        Ok(match input {
            ToolCatalog::Read(input) => {
                let normalized_path = self.normalize_path(input.file_path.clone());
                let output = self
                    .services
                    .read(
                        normalized_path,
                        input
                            .range
                            .as_ref()
                            .and_then(|r| r.start_line)
                            .map(|i| i as u64),
                        input
                            .range
                            .as_ref()
                            .and_then(|r| r.end_line)
                            .map(|i| i as u64),
                    )
                    .await?;

                (input, output).into()
            }
            ToolCatalog::Write(input) => {
                let normalized_path = self.normalize_path(input.file_path.clone());
                let output = self
                    .services
                    .write(normalized_path, input.content.clone(), input.overwrite)
                    .await?;
                (input, output).into()
            }
            ToolCatalog::FsSearch(input) => {
                let mut params = input.clone();
                // Normalize path if provided
                if let Some(ref path) = params.path {
                    params.path = Some(self.normalize_path(path.clone()));
                }
                let output = self.services.search(params).await?;
                (input, output).into()
            }
            ToolCatalog::SemSearch(input) => {
                let config = self.services.get_config()?;
                let env = self.services.get_environment();
                let services = self.services.clone();
                let cwd = env.cwd.clone();
                let limit = config.max_sem_search_results;
                let top_k = config.sem_search_top_k as u32;
                let params: Vec<_> = input
                    .queries
                    .iter()
                    .map(|search_query| {
                        forge_domain::SearchParams::new(&search_query.query, &search_query.use_case)
                            .limit(limit)
                            .top_k(top_k)
                    })
                    .collect();

                // Execute all queries in parallel
                let futures: Vec<_> = params
                    .into_iter()
                    .map(|param| services.query_workspace(cwd.clone(), param))
                    .collect();

                let mut results = futures::future::try_join_all(futures).await?;

                // Deduplicate results across queries
                crate::search_dedup::deduplicate_results(&mut results);

                let output = input
                    .queries
                    .into_iter()
                    .zip(results)
                    .map(|(query, results)| CodebaseQueryResult {
                        query: query.query,
                        use_case: query.use_case,
                        results,
                    })
                    .collect::<Vec<_>>();

                let output = forge_domain::CodebaseSearchResults { queries: output };
                ToolOperation::CodebaseSearch { output }
            }
            ToolCatalog::Remove(input) => {
                let normalized_path = self.normalize_path(input.path.clone());
                let output = self.services.remove(normalized_path).await?;
                (input, output).into()
            }
            ToolCatalog::Patch(input) => {
                let normalized_path = self.normalize_path(input.file_path.clone());
                let output = self
                    .services
                    .patch(
                        normalized_path,
                        input.old_string.clone(),
                        input.new_string.clone(),
                        input.replace_all,
                    )
                    .await?;
                (input, output).into()
            }
            ToolCatalog::MultiPatch(input) => {
                let normalized_path = self.normalize_path(input.file_path.clone());
                let output = self
                    .services
                    .multi_patch(normalized_path, input.edits.clone())
                    .await?;
                (input, output).into()
            }
            ToolCatalog::Undo(input) => {
                let normalized_path = self.normalize_path(input.path.clone());
                let output = self.services.undo(normalized_path).await?;
                (input, output).into()
            }
            ToolCatalog::Shell(input) => {
                let cwd = input
                    .cwd
                    .clone()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| self.services.get_environment().cwd.display().to_string());
                let normalized_cwd = self.normalize_path(cwd);

                // P1.1: route shell calls through forge_sandbox when configured
                // Convert env (Option<Vec<String>>) -> HashMap for SandboxConfig
                let env_map: std::collections::HashMap<String, String> = input
                    .env
                    .as_ref()
                    .map(|pairs| {
                        pairs
                            .iter()
                            .filter_map(|kv| {
                                let (k, v) = kv.split_once('=')?;
                                Some((k.to_string(), v.to_string()))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let output = if let Some(policy) = self.sandbox_policy.as_ref() {
                    self.execute_shell_sandboxed(
                        input.command.clone(),
                        PathBuf::from(normalized_cwd),
                        env_map,
                        input.keep_ansi,
                        policy,
                    )
                    .await?
                } else {
                    self.services
                        .execute(
                            input.command.clone(),
                            PathBuf::from(normalized_cwd),
                            input.keep_ansi,
                            false,
                            input.env.clone(),
                            input.description.clone(),
                        )
                        .await?
                };
                output.into()
            }
            ToolCatalog::Fetch(input) => {
                let output = self.services.fetch(input.url.clone(), input.raw).await?;
                (input, output).into()
            }
            ToolCatalog::Followup(input) => {
                let output = self
                    .services
                    .follow_up(
                        input.question.clone(),
                        input
                            .option1
                            .clone()
                            .into_iter()
                            .chain(input.option2.clone())
                            .chain(input.option3.clone())
                            .chain(input.option4.clone())
                            .chain(input.option5.clone())
                            .collect(),
                        input.multiple,
                    )
                    .await?;
                output.into()
            }
            ToolCatalog::Plan(input) => {
                let output = self
                    .services
                    .create_plan(
                        input.plan_name.clone(),
                        input.version.clone(),
                        input.content.clone(),
                    )
                    .await?;
                (input, output).into()
            }
            ToolCatalog::Skill(input) => {
                let skill = self.services.fetch_skill(input.name.clone()).await?;
                ToolOperation::Skill { output: skill }
            }
            ToolCatalog::TodoWrite(input) => {
                let before = context.get_todos()?;
                context.update_todos(input.todos.clone())?;
                let after = context.get_todos()?;
                ToolOperation::TodoWrite { before, after }
            }
            ToolCatalog::TodoRead(_input) => {
                let todos = context.get_todos()?;
                ToolOperation::TodoRead { output: todos }
            }
            ToolCatalog::Task(_) => {
                // Task tools are handled in ToolRegistry before reaching here
                unreachable!("Task tool should be handled in ToolRegistry")
            }
        })
    }

    #[tracing::instrument(skip(self, context), fields(tool = %tool_input.kind()))]
    pub async fn execute(
        &self,
        tool_input: ToolCatalog,
        context: &ToolCallContext,
    ) -> anyhow::Result<ToolOutput> {
        let tool_kind = tool_input.kind();
        let env = self.services.get_environment();
        let config = self.services.get_config()?;

        // Enforce read-before-edit for patch operations
        let file_path = match &tool_input {
            ToolCatalog::Patch(input) => Some(&input.file_path),
            ToolCatalog::MultiPatch(input) => Some(&input.file_path),
            _ => None,
        };

        if let Some(path) = file_path {
            self.require_prior_read(context, path, "edit it")?;
        }

        // Enforce read-before-edit for overwrite writes
        if let ToolCatalog::Write(input) = &tool_input
            && input.overwrite
        {
            self.require_prior_read(context, &input.file_path, "overwrite it")?;
        }

        let intent = if let (ToolCatalog::Write(input), Some(recovery)) =
            (&tool_input, self.effect_recovery.as_ref())
        {
            let conversation = context
                .conversation_id()
                .map(|id| format!("{id:?}"))
                .unwrap_or_else(|| "no-conversation".to_string());
            Some(EffectIntent {
                effect_id: format!(
                    "{}:{}:write:{}",
                    recovery.durable_effort_ref, conversation, input.file_path
                ),
                durable_effort_ref: recovery.durable_effort_ref.clone(),
                worker_attempt_id: recovery.worker_attempt_id.clone(),
                tool_name: "write".to_string(),
                target_fingerprint: input.file_path.clone(),
            })
        } else {
            None
        };

        let execution_result = execute_effect(
            self.effect_recovery.as_ref(),
            intent,
            || self.call_internal(tool_input.clone(), context),
        )
        .await;

        if let Err(ref error) = execution_result {
            tracing::error!(error = ?error, "Tool execution failed");
        }

        let operation = execution_result?;

        // Send formatted output message
        if let Some(output) = operation.to_content(&env) {
            context.send(output).await?;
        }

        let truncation_path = self.dump_operation(&operation).await?;

        context.with_metrics(|metrics| {
            operation.into_tool_output(tool_kind, truncation_path, &env, &config, metrics)
        })
    }
}

/// Minimal shlex-style command splitter for the sandbox path.
///
/// Splits on whitespace; supports single or double quoted segments; backslash
/// escapes the next character. Sufficient for the vast majority of agent shell
/// calls and avoids pulling a shlex dep. The intent is that the caller's
/// `Shell { command, .. }` is a single program + args invocation — anything
/// more complex (pipes, redirects, globs) keeps using the legacy bare-shell
/// path until explicitly opted in.
fn parse_shell_command(input: &str) -> (String, Vec<String>) {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escape = false;
    for c in input.chars() {
        if escape {
            cur.push(c);
            escape = false;
            continue;
        }
        match c {
            '\\' if in_single => {
                cur.push('\\');
            }
            '\\' => {
                escape = true;
            }
            '\'' if !in_double => {
                in_single = !in_single;
            }
            '"' if !in_single => {
                in_double = !in_double;
            }
            c if (c == ' ' || c == '\t') && !in_single && !in_double => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    if out.is_empty() {
        return ("".to_string(), Vec::new());
    }
    let program = out.remove(0);
    (program, out)
}


#[cfg(test)]
mod effect_recovery_tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingAdapter {
        events: Mutex<Vec<String>>,
        fail_confirm: bool,
    }

    #[async_trait]
    impl EffectRecoveryAdapter for RecordingAdapter {
        async fn begin(&self, intent: EffectIntent) -> anyhow::Result<()> {
            self.events.lock().unwrap().push(format!("intent:{}", intent.effect_id));
            Ok(())
        }

        async fn mark_dispatched(&self, effect_id: &str) -> anyhow::Result<()> {
            self.events.lock().unwrap().push(format!("dispatched:{effect_id}"));
            Ok(())
        }

        async fn confirm_success(&self, effect_id: &str) -> anyhow::Result<()> {
            self.events.lock().unwrap().push(format!("confirm:{effect_id}"));
            if self.fail_confirm {
                anyhow::bail!("simulated durable confirmation failure");
            }
            Ok(())
        }

        async fn mark_uncertain(&self, effect_id: &str, _reason: &str) -> anyhow::Result<()> {
            self.events.lock().unwrap().push(format!("uncertain:{effect_id}"));
            Ok(())
        }
    }

    fn intent() -> EffectIntent {
        EffectIntent {
            effect_id: "effect-1".into(),
            durable_effort_ref: "effort-1".into(),
            worker_attempt_id: "attempt-a".into(),
            tool_name: "write".into(),
            target_fingerprint: "fixture.txt".into(),
        }
    }

    #[tokio::test]
    async fn effect_wrapper_records_intent_dispatch_and_confirmation() {
        let adapter = Arc::new(RecordingAdapter::default());
        let recovery = EffectRecoveryContext {
            durable_effort_ref: "effort-1".into(),
            worker_attempt_id: "attempt-a".into(),
            adapter: adapter.clone(),
        };
        let side_effect = Arc::new(Mutex::new(0u8));
        let observed = Arc::clone(&side_effect);
        let result = execute_effect(Some(&recovery), Some(intent()), move || async move {
            *observed.lock().unwrap() += 1;
            Ok::<_, anyhow::Error>("done")
        })
        .await
        .unwrap();

        assert_eq!(result, "done");
        assert_eq!(*side_effect.lock().unwrap(), 1);
        assert_eq!(
            adapter.events.lock().unwrap().as_slice(),
            ["intent:effect-1", "dispatched:effect-1", "confirm:effect-1"]
        );
    }

    #[tokio::test]
    async fn effect_wrapper_marks_uncertain_after_effect_when_confirmation_fails() {
        let adapter = Arc::new(RecordingAdapter {
            fail_confirm: true,
            ..Default::default()
        });
        let recovery = EffectRecoveryContext {
            durable_effort_ref: "effort-1".into(),
            worker_attempt_id: "attempt-a".into(),
            adapter: adapter.clone(),
        };
        let side_effect = Arc::new(Mutex::new(0u8));
        let observed = Arc::clone(&side_effect);
        let error = execute_effect(Some(&recovery), Some(intent()), move || async move {
            *observed.lock().unwrap() += 1;
            Ok::<_, anyhow::Error>("committed")
        })
        .await
        .expect_err("failed durable confirmation must not return normal success");

        assert!(error.to_string().contains("durable confirmation failed"));
        assert_eq!(*side_effect.lock().unwrap(), 1, "effect already committed");
        assert_eq!(
            adapter.events.lock().unwrap().as_slice(),
            [
                "intent:effect-1",
                "dispatched:effect-1",
                "confirm:effect-1",
                "uncertain:effect-1"
            ]
        );
    }
}
