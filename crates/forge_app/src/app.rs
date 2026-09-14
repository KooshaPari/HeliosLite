use std::sync::Arc;

use anyhow::Result;
use chrono::Local;
use forge_config::ForgeConfig;
use forge_domain::*;
use forge_stream::MpscStream;

use crate::apply_tunable_parameters::ApplyTunableParameters;
use crate::changed_files::ChangedFiles;
use crate::dto::ToolsOverview;
use crate::hooks::{
    AutoRepairHook, CompactionHandler, DoomLoopDetector, PendingTodosHandler, SandboxHook,
    TitleGenerationHandler, TracingHandler,
};
use crate::init_conversation_metrics::InitConversationMetrics;
use crate::orch::Orchestrator;
use crate::services::{AgentRegistry, CustomInstructionsService, ProviderAuthService};
use crate::set_conversation_id::SetConversationId;
use crate::system_prompt::SystemPrompt;
use crate::tool_registry::ToolRegistry;
use crate::tool_resolver::ToolResolver;
use crate::user_prompt::UserPromptGenerator;
use crate::{
    AgentExt, AgentProviderResolver, ConversationService, EnvironmentInfra, FileDiscoveryService,
    ProviderService, Services, WorkspaceService,
};

/// Builds a [`TemplateConfig`] from a [`ForgeConfig`].
///
/// Converts the configuration-layer field names into the domain-layer struct
/// expected by [`SystemContext`] for tool description template rendering.
pub(crate) fn build_template_config(config: &ForgeConfig) -> forge_domain::TemplateConfig {
    forge_domain::TemplateConfig {
        max_read_size: config.max_read_lines as usize,
        max_line_length: config.max_line_chars,
        max_image_size: config.max_image_size_bytes as usize,
        stdout_max_prefix_length: config.max_stdout_prefix_lines,
        stdout_max_suffix_length: config.max_stdout_suffix_lines,
        stdout_max_line_length: config.max_stdout_line_chars,
    }
}

/// Stores the conversation summary in the semantic-memory backend (best-effort).
///
/// This is wired into the chat-completion path so that every conversation whose
/// underlying [`ConversationService::upsert_conversation`] succeeds is also
/// reflected in the semantic-memory adapter selected by
/// `FORGE_SEMANTIC_ADAPTER` (JSONL by default; Supermemory / Letta / Cognee
/// when configured). A failure here is logged and swallowed so the chat
/// completion path is never broken by a memory-side error.
async fn store_conversation_in_semantic_memory<S>(
    services: &Arc<S>,
    conversation: &forge_domain::Conversation,
) where
    S: Services + EnvironmentInfra<Config = ForgeConfig>,
{
    let services: &S = services.as_ref();
    // Skip empty conversations — there's nothing meaningful to index.
    if conversation
        .title
        .as_deref()
        .unwrap_or("")
        .trim()
        .is_empty()
    {
        return;
    }

    // Resolve the workspace_id from the conversation's cwd (or fall back to
    // the runtime environment cwd). init_workspace is idempotent — it returns
    // the existing workspace for the path if one is already known.
    let cwd = conversation
        .cwd
        .as_deref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| services.get_environment().cwd);

    let workspace_id = match services.init_workspace(cwd.clone()).await {
        Ok(id) => id,
        Err(err) => {
            tracing::warn!(
                workspace = %cwd.display(),
                error = %err,
                "skipping semantic-memory store; could not resolve workspace_id",
            );
            return;
        }
    };

    let namespace = forge_domain::SemanticMemoryNamespace::new(workspace_id);
    let provenance = forge_domain::SemanticMemoryProvenance::new(
        conversation.id,
        namespace.clone(),
        format!("conversation:{}", conversation.id),
    );

    // The write payload is the human-readable title + a short text digest of
    // the context. The full ConversationContext isn't deserialised here to
    // keep the index lightweight — adapters that need richer payloads can
    // pull them on recall.
    let message_count = conversation.message_count.unwrap_or(0);
    let content = format!(
        "title: {}\nmessages: {}\ncwd: {}",
        conversation.title.as_deref().unwrap_or(""),
        message_count,
        cwd.display(),
    );

    let write = match forge_domain::SemanticMemoryWrite::new(
        forge_domain::SemanticMemoryScope::Episodic,
        namespace,
        content,
        provenance,
    ) {
        Ok(w) => w,
        Err(err) => {
            tracing::warn!(error = %err, "skipping semantic-memory store; invalid write");
            return;
        }
    };

    if let Err(err) = services.semantic_memory_service().store(write).await {
        tracing::warn!(
            conversation_id = %conversation.id,
            error = %err,
            "semantic-memory store failed; conversation is still saved",
        );
    }
}

/// Best-effort recall of recent episodic memories from the semantic-memory
/// adapter for the current workspace, returned as a context digest suitable
/// for prefixing the system prompt.
///
/// Returns `None` if the workspace can't be resolved, the query is invalid,
/// the adapter errors, or the adapter returns no records — any failure path
/// is logged via `tracing::warn!` and the caller proceeds without recall.
///
/// This is invoked once per `chat()` invocation, before `SystemPrompt` runs,
/// so the model sees the most recent related work the user has done in this
/// workspace.
async fn recall_workspace_episodic_context<S>(services: &Arc<S>, query_text: &str) -> Option<String>
where
    S: Services + EnvironmentInfra<Config = ForgeConfig>,
{
    let services: &S = services.as_ref();
    let environment = services.get_environment();
    let cwd = environment.cwd.clone();

    let workspace_id = match services.init_workspace(cwd.clone()).await {
        Ok(id) => id,
        Err(err) => {
            tracing::warn!(
                workspace = %cwd.display(),
                error = %err,
                "skipping semantic-memory recall; could not resolve workspace_id",
            );
            return None;
        }
    };

    let namespace = forge_domain::SemanticMemoryNamespace::new(workspace_id);
    let limit = forge_domain::SemanticMemoryQuery::DEFAULT_LIMIT;
    let query = match forge_domain::SemanticMemoryQuery::new(namespace, query_text, limit, None) {
        Ok(q) => q,
        Err(err) => {
            tracing::warn!(error = %err, "skipping semantic-memory recall; invalid query");
            return None;
        }
    };
    let budget = match forge_domain::SemanticMemoryBudget::new(
        forge_domain::SemanticMemoryBudget::DEFAULT_BYTES,
    ) {
        Ok(b) => b,
        Err(err) => {
            tracing::warn!(error = %err, "skipping semantic-memory recall; invalid budget");
            return None;
        }
    };

    match services
        .semantic_memory_service()
        .recall(query, budget)
        .await
    {
        Ok(records) if records.is_empty() => None,
        Ok(records) => {
            let digest: Vec<String> = records
                .into_iter()
                .map(|r| {
                    format!(
                        "- [{}] {}",
                        r.provenance().conversation_id(),
                        r.content().lines().next().unwrap_or("").trim()
                    )
                })
                .collect();
            Some(format!(
                "Recent related work in this workspace ({} entries):\n{}",
                digest.len(),
                digest.join("\n")
            ))
        }
        Err(err) => {
            tracing::warn!(
                error = %err,
                "semantic-memory recall failed; continuing without recall",
            );
            None
        }
    }
}

/// Forgets the conversation's semantic-memory record (best-effort). Wired
/// into the compaction path so a compacted conversation no longer keeps its
/// pre-compaction entry in the recall index.
async fn forget_conversation_in_semantic_memory<S>(
    services: &Arc<S>,
    conversation: &forge_domain::Conversation,
) where
    S: Services + EnvironmentInfra<Config = ForgeConfig>,
{
    let services: &S = services.as_ref();
    let cwd = conversation
        .cwd
        .as_deref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| services.get_environment().cwd);

    let workspace_id = match services.init_workspace(cwd.clone()).await {
        Ok(id) => id,
        Err(err) => {
            tracing::warn!(
                workspace = %cwd.display(),
                error = %err,
                "skipping semantic-memory forget; could not resolve workspace_id",
            );
            return;
        }
    };

    let identity = match forge_domain::SemanticMemoryIdentity::new(
        forge_domain::SemanticMemoryScope::Episodic,
        forge_domain::SemanticMemoryNamespace::new(workspace_id),
        format!("conversation:{}", conversation.id),
    ) {
        Ok(id) => id,
        Err(err) => {
            tracing::warn!(
                conversation_id = %conversation.id,
                error = %err,
                "skipping semantic-memory forget; invalid identity",
            );
            return;
        }
    };

    if let Err(err) = services.semantic_memory_service().forget(identity).await {
        tracing::warn!(
            conversation_id = %conversation.id,
            error = %err,
            "semantic-memory forget failed; continuing without deleting the record",
        );
    }
}

/// ForgeApp handles the core chat functionality by orchestrating various
/// services. It encapsulates the complex logic previously contained in the
/// ForgeAPI chat method.
pub struct ForgeApp<S> {
    services: Arc<S>,
    tool_registry: ToolRegistry<S>,
}

impl<S: Services + EnvironmentInfra<Config = forge_config::ForgeConfig>> ForgeApp<S> {
    /// Creates a new ForgeApp instance with the provided services.
    pub fn new(services: Arc<S>) -> anyhow::Result<Self> {
        Ok(Self {
            tool_registry: ToolRegistry::new(services.clone())?,
            services,
        })
    }

    /// Executes a chat request and returns a stream of responses.
    /// This method contains the core chat logic extracted from ForgeAPI.
    pub async fn chat(
        &self,
        agent_id: AgentId,
        chat: ChatRequest,
    ) -> Result<MpscStream<Result<ChatResponse, anyhow::Error>>> {
        let services = self.services.clone();

        // Get the conversation for the chat request
        let conversation = services
            .find_conversation(&chat.conversation_id)
            .await?
            .ok_or_else(|| forge_domain::Error::ConversationNotFound(chat.conversation_id))?;

        // Discover files using the discovery service
        let forge_config = self.services.get_config()?;
        let environment = services.get_environment();

        let files = services.list_current_directory().await?;

        let custom_instructions = services.get_custom_instructions().await;

        // Warm-hydrate: recall recent episodic memories for this workspace
        // before the model is invoked. The digest is appended to the system
        // prompt's custom instructions so the model sees prior work without
        // a round-trip. Best-effort — empty / errored recall is a no-op.
        // Query text is a stable, generic anchor — the workspace path itself
        // — so the recall surfaces any prior conversations done here.
        let query_text: String = services
            .get_environment()
            .cwd
            .to_string_lossy()
            .into_owned();
        let recalled = recall_workspace_episodic_context(&self.services, &query_text).await;
        let mut custom_instructions = custom_instructions;
        if let Some(recall) = recalled {
            if !recall.is_empty() {
                let combined = if custom_instructions.is_empty() {
                    recall
                } else {
                    let existing = custom_instructions.join("\n\n");
                    format!("{}\n\n{}", existing, recall)
                };
                custom_instructions = vec![combined];
            }
        }

        // Prepare agents with user configuration
        let agent_provider_resolver = AgentProviderResolver::new(services.clone());

        // Get agent and apply workflow config
        let agent = self
            .services
            .get_agent(&agent_id)
            .await?
            .ok_or(crate::Error::AgentNotFound(agent_id.clone()))?
            .apply_config(&forge_config)
            .set_compact_model_if_none();

        let agent_provider = agent_provider_resolver
            .get_provider(Some(agent.id.clone()))
            .await?;
        let agent_provider = self
            .services
            .provider_auth_service()
            .refresh_provider_credential(agent_provider)
            .await?;

        let models = services.models(agent_provider).await?;
        let selected_model = models.iter().find(|model| model.id == agent.model);
        let agent = agent.compaction_threshold(selected_model);

        // Get system and mcp tool definitions and resolve them for the agent
        let all_tool_definitions = self.tool_registry.list().await?;
        let tool_resolver = ToolResolver::new(all_tool_definitions);
        let tool_definitions: Vec<ToolDefinition> =
            tool_resolver.resolve(&agent).into_iter().cloned().collect();
        let max_tool_failure_per_turn = agent.max_tool_failure_per_turn.unwrap_or(3);

        let current_time = Local::now();

        // Insert system prompt
        let conversation =
            SystemPrompt::new(self.services.clone(), environment.clone(), agent.clone())
                .custom_instructions(custom_instructions.clone())
                .tool_definitions(tool_definitions.clone())
                .models(models.clone())
                .files(files.clone())
                .max_extensions(forge_config.max_extensions)
                .template_config(build_template_config(&forge_config))
                .add_system_message(conversation)
                .await?;

        // Insert user prompt
        let conversation = UserPromptGenerator::new(
            self.services.clone(),
            agent.clone(),
            chat.event.clone(),
            current_time,
        )
        .add_user_prompt(conversation)
        .await?;

        // Detect and render externally changed files notification
        let conversation = ChangedFiles::new(services.clone(), agent.clone())
            .update_file_stats(conversation)
            .await;

        let conversation = InitConversationMetrics::new(current_time).apply(conversation);
        let conversation = ApplyTunableParameters::new(agent.clone(), tool_definitions.clone())
            .apply(conversation);
        let conversation = SetConversationId.apply(conversation);

        // Create the orchestrator with all necessary dependencies
        let tracing_handler = TracingHandler::new();
        let title_handler = TitleGenerationHandler::new(services.clone());

        // Build the on_end hook, conditionally adding PendingTodosHandler based on
        // config
        let on_end_hook = if forge_config.verify_todos {
            tracing_handler
                .clone()
                .and(title_handler.clone())
                .and(PendingTodosHandler::new())
        } else {
            tracing_handler.clone().and(title_handler.clone())
        };

        let hook = Hook::default()
            .on_start(tracing_handler.clone().and(title_handler))
            .on_request(tracing_handler.clone().and(DoomLoopDetector::default()))
            .on_response(
                tracing_handler
                    .clone()
                    .and(CompactionHandler::new(agent.clone(), environment.clone())),
            )
            .on_toolcall_start(tracing_handler.clone())
            .on_toolcall_end(
                tracing_handler
                    .clone()
                    .and(AutoRepairHook::new())
                    .and(SandboxHook::new()),
            )
            .on_end(on_end_hook);

        let orch = Orchestrator::new(
            services.clone(),
            conversation,
            agent,
            self.services.get_config()?,
        )
        .error_tracker(ToolErrorTracker::new(max_tool_failure_per_turn))
        .tool_definitions(tool_definitions)
        .models(models)
        .hook(Arc::new(hook));

        // Create and return the stream
        let stream = MpscStream::spawn(
            |tx: tokio::sync::mpsc::Sender<Result<ChatResponse, anyhow::Error>>| {
                async move {
                    // Execute dispatch and always save conversation afterwards
                    let mut orch = orch.sender(tx.clone());
                    let dispatch_result = orch.run().await;

                    // Always save conversation using get_conversation()
                    let conversation = orch.get_conversation().clone();
                    let save_result = services.upsert_conversation(conversation.clone()).await;

                    // Best-effort: mirror the saved conversation into the
                    // semantic-memory backend so the adapter selected by
                    // FORGE_SEMANTIC_ADAPTER (JSONL by default; Supermemory /
                    // Letta / Cognee when configured) reflects it. Failures are
                    // logged and never break the chat-completion path.
                    store_conversation_in_semantic_memory(&services, &conversation).await;

                    // Send any error to the stream (prioritize dispatch error over save error)
                    #[allow(clippy::collapsible_if)]
                    if let Some(err) = dispatch_result.err().or(save_result.err()) {
                        if let Err(e) = tx.send(Err(err)).await {
                            tracing::error!("Failed to send error to stream: {}", e);
                        }
                    }
                }
            },
        );

        Ok(stream)
    }

    /// Compacts the context of the main agent for the given conversation and
    /// persists it. Returns metrics about the compaction (original vs.
    /// compacted tokens and messages).
    pub async fn compact_conversation(
        &self,
        active_agent_id: AgentId,
        conversation_id: &ConversationId,
    ) -> Result<CompactionResult> {
        use crate::compact::Compactor;

        // Get the conversation
        let mut conversation = self
            .services
            .find_conversation(conversation_id)
            .await?
            .ok_or_else(|| forge_domain::Error::ConversationNotFound(*conversation_id))?;

        // Get the context from the conversation
        let context = match conversation.context.as_ref() {
            Some(context) => context.clone(),
            None => {
                // No context to compact, return zero metrics
                return Ok(CompactionResult::new(0, 0, 0, 0));
            }
        };

        // Calculate original metrics
        let original_messages = context.messages.len();
        let original_token_count = *context.token_count();

        let forge_config = self.services.get_config()?;

        // Get agent and apply workflow config
        let agent = self.services.get_agent(&active_agent_id).await?;

        let Some(agent) = agent else {
            return Ok(CompactionResult::new(
                original_token_count,
                0,
                original_messages,
                0,
            ));
        };

        // Get compact config from the agent
        let compact = agent
            .apply_config(&forge_config)
            .set_compact_model_if_none()
            .compact;

        // Apply compaction using the Compactor
        let environment = self.services.get_environment();
        let compacted_context = Compactor::new(compact, environment).compact(context, true)?;

        let compacted_messages = compacted_context.messages.len();
        let compacted_tokens = *compacted_context.token_count();

        // Update the conversation with the compacted context
        conversation.context = Some(compacted_context);

        // Save the updated conversation
        self.services
            .upsert_conversation(conversation.clone())
            .await?;

        // Best-effort: remove the pre-compaction semantic-memory record so the
        // recall index doesn't keep returning stale context for this id.
        forget_conversation_in_semantic_memory(&self.services, &conversation).await;

        Ok(CompactionResult::new(
            original_token_count,
            compacted_tokens,
            original_messages,
            compacted_messages,
        ))
    }

    pub async fn list_tools(&self) -> Result<ToolsOverview> {
        self.tool_registry.tools_overview().await
    }

    /// Gets available models for the default provider with automatic credential
    /// refresh.
    pub async fn get_models(&self) -> Result<Vec<Model>> {
        let agent_provider_resolver = AgentProviderResolver::new(self.services.clone());
        let provider = agent_provider_resolver.get_provider(None).await?;
        let provider = self
            .services
            .provider_auth_service()
            .refresh_provider_credential(provider)
            .await?;

        self.services.models(provider).await
    }

    /// Gets available models from all configured providers concurrently.
    ///
    /// Returns a list of `ProviderModels` for each configured provider that
    /// successfully returned models. If every configured provider fails (e.g.
    /// due to an invalid API key), the first error encountered is returned so
    /// the caller receives the real underlying cause rather than an empty list.
    pub async fn get_all_provider_models(&self) -> Result<Vec<ProviderModels>> {
        let all_providers = self.services.get_all_providers().await?;

        // Build one future per configured provider, preserving the error on failure.
        let futures: Vec<_> = all_providers
            .into_iter()
            .filter_map(|any_provider| any_provider.into_configured())
            .map(|provider| {
                let provider_id = provider.id.clone();
                let services = self.services.clone();
                async move {
                    let result: Result<ProviderModels> = async {
                        let refreshed = services
                            .provider_auth_service()
                            .refresh_provider_credential(provider)
                            .await?;
                        let models = services.models(refreshed).await?;
                        Ok(ProviderModels { provider_id, models })
                    }
                    .await;
                    result
                }
            })
            .collect();

        // Execute all provider fetches concurrently.
        let results: Vec<Result<ProviderModels>> = futures::future::join_all(futures).await;

        // Separate successes from failures, logging each failure.
        let mut models = Vec::with_capacity(results.len());
        let mut first_error: Option<anyhow::Error> = None;

        for result in results {
            match result {
                Ok(provider_models) => models.push(provider_models),
                Err(err) => {
                    tracing::warn!("Failed to fetch models for a provider: {err}");
                    if first_error.is_none() {
                        first_error = Some(err);
                    }
                }
            }
        }

        // If every provider failed, surface the first error so the caller
        // knows why rather than seeing a confusing empty list.
        if models.is_empty()
            && let Some(err) = first_error
        {
            return Err(err);
        }

        Ok(models)
    }
}
