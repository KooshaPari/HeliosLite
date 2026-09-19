use crate::jobs::{self, ReleaseBuilderJob};
use crate::steps::setup_protoc;
use crate::workflow_model::{Event, Job, Level, Permissions, Push, Step, Workflow};

// Inspect the changed label, not the current label list: unrelated label edits
// must not rerun CI even when the all-targets opt-in label is already present.
const CI_EVENT_GUARD: &str = "github.event_name != 'pull_request' || (github.event.action != 'labeled' && github.event.action != 'unlabeled') || github.event.label.name == 'ci: build all targets'";

/// Regenerate the CI workflow from its private workflow model.
pub fn generate_ci_workflow() {
    super::generate_private_workflow(ci_workflow(), "ci.yml");
}

fn ci_workflow() -> Workflow {
    let pr_build_guard = format!(
        "github.event_name == 'pull_request' && contains(github.event.pull_request.labels.*.name, 'ci: build all targets') && ({CI_EVENT_GUARD})"
    );
    let build_job = Job::new("Build and Test")
        .if_condition(CI_EVENT_GUARD)
        .permissions(Permissions::default().contents(Level::Read))
        .add_step(Step::new("Checkout Code").uses(
            "actions",
            "checkout",
            "d23441a48e516b6c34aea4fa41551a30e30af803",
        ))
        .add_step(setup_protoc())
        .add_step(
            Step::new("Setup Rust Toolchain")
                .uses(
                    "actions-rust-lang",
                    "setup-rust-toolchain",
                    "166cdcfd11aee3cb47222f9ddb555ce30ddb9659",
                )
                .input("toolchain", "stable"),
        )
        .add_step(Step::new("Install cargo-llvm-cov").run("cargo install cargo-llvm-cov"))
        .add_step(
            Step::new("Generate coverage")
                .run("cargo llvm-cov --all-features --workspace --lcov --output-path lcov.info"),
        );
    let perf_test_job = Job::new("Performance: zsh rprompt")
        .if_condition(CI_EVENT_GUARD)
        .permissions(Permissions::default().contents(Level::Read))
        .add_step(Step::new("Checkout Code").uses(
            "actions",
            "checkout",
            "d23441a48e516b6c34aea4fa41551a30e30af803",
        ))
        .add_step(setup_protoc())
        .add_step(
            Step::new("Setup Rust Toolchain")
                .uses(
                    "actions-rust-lang",
                    "setup-rust-toolchain",
                    "166cdcfd11aee3cb47222f9ddb555ce30ddb9659",
                )
                .input("toolchain", "stable"),
        )
        .add_step(
            Step::new("Run performance benchmark")
                .run("./scripts/benchmark.sh --threshold 60 zsh rprompt"),
        );
    let draft_release_job = jobs::create_draft_release_job("build");
    let draft_release_pr_job = jobs::create_draft_release_pr_job().if_condition(&pr_build_guard);
    let build_release_pr_job =
        ReleaseBuilderJob::new("${{ needs.draft_release_pr.outputs.crate_release_name }}")
            .into_job()
            .needs("draft_release_pr")
            .if_condition(&pr_build_guard);
    let events = Event::default()
        .push(Push::default().add_branch("main").add_tag("v*"))
        .pull_request(
            ["opened", "synchronize", "reopened", "labeled", "unlabeled"],
            ["main", "integration/**"],
        );
    Workflow::new("ci")
        .env("RUSTFLAGS", "-Dwarnings")
        .env("OPENROUTER_API_KEY", "${{secrets.OPENROUTER_API_KEY}}")
        .on(events)
        .concurrency("${{ github.workflow }}-${{ github.ref }}", false)
        .permissions(Permissions::default().contents(Level::Read))
        .add_job("build", build_job)
        .add_job("zsh_rprompt_perf", perf_test_job)
        .add_job("draft_release", draft_release_job)
        .add_job("draft_release_pr", draft_release_pr_job)
        .add_job("build_release_pr", build_release_pr_job)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde_yaml_ng::Value;

    use super::ci_workflow;

    fn fixture() -> Result<Value, serde_yaml_ng::Error> {
        serde_yaml_ng::from_str(&ci_workflow().to_yaml()?)
    }

    #[test]
    fn generated_ci_matches_workflow_snapshot() {
        let fixture = fixture().unwrap();
        let actual = fixture;
        let expected: Value =
            serde_yaml_ng::from_str(include_str!("../../../../.github/workflows/ci.yml")).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn expensive_jobs_have_the_changed_label_guard() {
        let fixture = fixture().unwrap();
        let actual = [
            "build",
            "zsh_rprompt_perf",
            "draft_release_pr",
            "build_release_pr",
        ]
        .map(|job| fixture["jobs"][job]["if"].as_str().unwrap());
        let guard = "github.event_name != 'pull_request' || (github.event.action != 'labeled' && github.event.action != 'unlabeled') || github.event.label.name == 'ci: build all targets'";
        let opt_in = format!(
            "github.event_name == 'pull_request' && contains(github.event.pull_request.labels.*.name, 'ci: build all targets') && ({guard})"
        );
        let expected = [guard, guard, opt_in.as_str(), opt_in.as_str()];
        assert_eq!(actual, expected);
    }

    #[test]
    fn ci_preserves_trigger_and_runner_policy() {
        let fixture = fixture().unwrap();
        let actual = (
            fixture["on"].clone(),
            fixture["jobs"]["build"]["runs-on"].clone(),
            fixture["jobs"]["zsh_rprompt_perf"]["runs-on"].clone(),
            fixture["concurrency"].clone(),
        );
        let expected = (
            serde_yaml_ng::from_str::<Value>(
                "push:\n  branches: [main]\n  tags: ['v*']\npull_request:\n  types: [opened, synchronize, reopened, labeled, unlabeled]\n  branches: [main, integration/**]\n",
            )
            .unwrap(),
            Value::from("ubuntu-latest"),
            Value::from("ubuntu-latest"),
            serde_yaml_ng::from_str::<Value>(
                "group: '${{ github.workflow }}-${{ github.ref }}'\ncancel-in-progress: false\n",
            )
            .unwrap(),
        );
        assert_eq!(actual, expected);
    }

    // These generated expressions use only string comparisons, parentheses,
    // &&, || and contains. After substituting fixture context and contains,
    // Bash [[ ]] evaluates this shared boolean subset without a custom parser.
    // Fixture values are static test data, never external event payloads.
    #[cfg(unix)]
    fn evaluate_condition(
        condition: &str,
        event: &str,
        action: &str,
        label: &str,
        opted_in: bool,
        git_ref: &str,
    ) -> bool {
        let expression = condition
            .replace(
                "contains(github.event.pull_request.labels.*.name, 'ci: build all targets')",
                if opted_in {
                    "'true' == 'true'"
                } else {
                    "'true' == 'false'"
                },
            )
            .replace("github.event_name", &format!("'{event}'"))
            .replace("github.event.action", &format!("'{action}'"))
            .replace("github.event.label.name", &format!("'{label}'"))
            .replace("github.ref", &format!("'{git_ref}'"));
        assert!(!expression.contains("github."));
        let output = std::process::Command::new("bash")
            .args(["-c", &format!("[[ {expression} ]]")])
            .output()
            .unwrap();
        assert!(output.stderr.is_empty(), "{expression}: {output:?}");
        assert!(matches!(output.status.code(), Some(0 | 1)));
        output.status.success()
    }

    #[test]
    #[cfg(unix)]
    fn unrelated_label_events_skip_all_ci_jobs_even_when_opted_in() {
        let fixture = fixture().unwrap();
        for action in ["labeled", "unlabeled"] {
            for label in ["bug", "documentation", "ci: build all targets extra", ""] {
                for opted_in in [false, true] {
                    let actual: Vec<_> = fixture["jobs"]
                        .as_mapping()
                        .unwrap()
                        .values()
                        .map(|job| {
                            evaluate_condition(
                                job["if"].as_str().unwrap(),
                                "pull_request",
                                action,
                                label,
                                opted_in,
                                "refs/pull/277/merge",
                            )
                        })
                        .collect();
                    let expected = vec![false; 5];
                    assert_eq!(actual, expected, "{action}, {label}, opted_in={opted_in}");
                }
            }
        }
    }

    #[test]
    #[cfg(unix)]
    fn ordinary_events_and_relevant_label_transitions_keep_expected_jobs() {
        let fixture = fixture().unwrap();
        let cases = [
            (
                "pull_request",
                "opened",
                "",
                false,
                "refs/pull/277/merge",
                [true, true, false, false, false],
            ),
            (
                "pull_request",
                "opened",
                "",
                true,
                "refs/pull/277/merge",
                [true, true, false, true, true],
            ),
            (
                "pull_request",
                "synchronize",
                "",
                false,
                "refs/pull/277/merge",
                [true, true, false, false, false],
            ),
            (
                "pull_request",
                "synchronize",
                "",
                true,
                "refs/pull/277/merge",
                [true, true, false, true, true],
            ),
            (
                "pull_request",
                "reopened",
                "",
                false,
                "refs/pull/277/merge",
                [true, true, false, false, false],
            ),
            (
                "pull_request",
                "reopened",
                "",
                true,
                "refs/pull/277/merge",
                [true, true, false, true, true],
            ),
            (
                "pull_request",
                "labeled",
                "ci: build all targets",
                true,
                "refs/pull/277/merge",
                [true, true, false, true, true],
            ),
            (
                "pull_request",
                "unlabeled",
                "ci: build all targets",
                false,
                "refs/pull/277/merge",
                [true, true, false, false, false],
            ),
            (
                "push",
                "",
                "",
                false,
                "refs/heads/main",
                [true, true, true, false, false],
            ),
            (
                "push",
                "",
                "",
                false,
                "refs/tags/v1.0.0",
                [true, true, false, false, false],
            ),
        ];
        for (event, action, label, opted_in, git_ref, expected) in cases {
            let actual = [
                "build",
                "zsh_rprompt_perf",
                "draft_release",
                "draft_release_pr",
                "build_release_pr",
            ]
            .map(|job| {
                evaluate_condition(
                    fixture["jobs"][job]["if"].as_str().unwrap(),
                    event,
                    action,
                    label,
                    opted_in,
                    git_ref,
                )
            });
            assert_eq!(actual, expected, "{event}, {action}, opted_in={opted_in}");
        }
    }
}
