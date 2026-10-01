use forge_ci::workflows as workflow;
use pretty_assertions::assert_eq;

const GENERATED_WORKFLOWS: [&str; 7] = [
    "autofix.yml",
    "bounty.yml",
    "ci.yml",
    "labels.yml",
    "release-drafter.yml",
    "release.yml",
    "stale.yml",
];

#[test]
fn signing_is_serialized_before_release_provenance() {
    let fixture = workflow::release_publish_yaml().unwrap();
    let actual: serde_yaml_ng::Value = serde_yaml_ng::from_str(&fixture).unwrap();
    let jobs = &actual["jobs"];
    assert_eq!(
        jobs["sign_release"]["needs"].as_str(),
        Some("build_release")
    );
    assert_eq!(
        jobs["sbom_release_assets"]["needs"].as_str(),
        Some("sign_release")
    );
    assert_eq!(
        jobs["attest_release_assets"]["needs"].as_str(),
        Some("sbom_release_assets")
    );
    assert_eq!(
        jobs["sign_release"]["uses"].as_str(),
        Some("./.github/workflows/sign-release.yml")
    );
    assert!(jobs["sign_release"]["runs-on"].is_null());
    let signing_source =
        std::fs::read_to_string(generated_workflow_path("sign-release.yml")).unwrap();
    assert!(signing_source.contains("api_present=0"));
    assert!(signing_source.contains("apple_present=0"));
    assert!(signing_source.contains("Incomplete API-key notarization configuration"));
    assert!(signing_source.contains("Incomplete Apple-ID notarization configuration"));
    assert!(signing_source.contains("NOTARY_AUTH_MODE=api-key"));
    assert!(signing_source.contains("notary-authkey.p8"));
    assert!(signing_source.contains("trap 'rm -f \"$NOTARY_KEY_PATH\"' EXIT"));
    let signing: serde_yaml_ng::Value = serde_yaml_ng::from_str(&signing_source).unwrap();
    assert!(
        signing["on"]
            .as_mapping()
            .unwrap()
            .contains_key("workflow_call")
    );
    assert!(!signing["on"].as_mapping().unwrap().contains_key("release"));
    assert_eq!(
        jobs["sign_release"]["with"]["tag"],
        "${{ github.event.release.tag_name }}"
    );
    assert_eq!(
        signing["on"]["workflow_call"]["inputs"]["tag"]["required"],
        true
    );
    assert_eq!(signing["jobs"]["sign"]["strategy"]["fail-fast"], false);
    let actual = jobs["sign_release"]["secrets"].as_mapping().unwrap();
    let expected = signing["on"]["workflow_call"]["secrets"]
        .as_mapping()
        .unwrap();
    assert_eq!(actual.len(), 11);
    assert_eq!(
        actual.keys().collect::<Vec<_>>(),
        expected.keys().collect::<Vec<_>>()
    );
    for (name, value) in actual {
        let expected = format!("${{{{ secrets.{} }}}}", name.as_str().unwrap());
        assert_eq!(value.as_str().unwrap(), expected);
    }
    for name in [
        "MACOS_NOTARIZATION_API_KEY",
        "MACOS_NOTARIZATION_KEY_ID",
        "MACOS_NOTARIZATION_ISSUER_ID",
    ] {
        assert!(expected.contains_key(name));
    }
}

fn generated_workflow_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(".github/workflows")
        .join(name)
}

#[test]
fn release_tags_reach_gh_as_literal_arguments() {
    let release: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&workflow::release_publish_yaml().unwrap()).unwrap();
    let signing: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(include_str!("../../../.github/workflows/sign-release.yml"))
            .unwrap();
    let tag = "v$(printf INJECTED)-`printf EXECUTED`-\"quoted\"";
    let mut release_commands = 0;
    for document in [&release, &signing] {
        for job in document["jobs"].as_mapping().unwrap().values() {
            let Some(steps) = job["steps"].as_sequence() else {
                continue;
            };
            for step in steps {
                let Some(script) = step["run"].as_str() else {
                    continue;
                };
                assert!(!script.contains("${{ inputs.tag }}"));
                assert!(!script.contains("${{ github.event.release.tag_name }}"));
                if !script.contains("gh release upload") {
                    continue;
                }
                assert!(script.contains("\"$RELEASE_TAG\""));
                assert!(step["env"]["RELEASE_TAG"].is_string());
                let script = script
                    .replace("${{ github.repository }}", "owner/repo")
                    .replace("${{ matrix.pattern }}", "*apple-darwin*");
                // Exercise the actual rendered shell without network or filesystem writes.
                let script = format!("mkdir() {{ :; }}\ngh() {{ printf '%s' \"$3\"; }}\n{script}");
                let bash = if cfg!(windows) {
                    std::path::PathBuf::from(
                        std::env::var_os("ProgramFiles").expect("Git Bash requires ProgramFiles"),
                    )
                    .join("Git/bin/bash.exe")
                } else {
                    std::path::PathBuf::from("bash")
                };
                let actual = std::process::Command::new(bash)
                    .args(["--noprofile", "--norc", "-c", &script])
                    .env("RELEASE_TAG", tag)
                    .output()
                    .unwrap();
                assert!(
                    actual.status.success(),
                    "bash fixture failed: {}",
                    std::str::from_utf8(&actual.stderr).unwrap_or("<non-UTF-8 stderr>")
                );
                assert_eq!(String::from_utf8(actual.stdout).unwrap(), tag);
                release_commands += 1;
            }
        }
    }
    assert_eq!(release_commands, 1);
}

#[test]
fn generated_workflows_are_parseable_and_identify_forge_ci_generator() {
    workflow::generate_autofix_workflow();
    workflow::generate_bounty_workflow();
    workflow::generate_ci_workflow();
    workflow::generate_labels_workflow();
    workflow::generate_release_drafter_workflow();
    workflow::generate_stale_workflow();

    for name in GENERATED_WORKFLOWS {
        let generated = std::fs::read_to_string(generated_workflow_path(name))
            .expect("generated workflow should exist");
        let parsed = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&generated);

        assert!(parsed.is_ok(), "{name} must remain valid YAML");
        assert!(
            generated.contains("forge_ci"),
            "{name} must identify forge_ci as its generator"
        );
        assert!(
            !generated.contains("gh-workflow"),
            "{name} must not identify gh-workflow"
        );
    }

    let release = std::fs::read_to_string(generated_workflow_path("release.yml"))
        .expect("generated release workflow");
    assert!(release.contains("attest_release_assets:"));
    assert!(release.contains("needs: build_release"));

    let ci =
        std::fs::read_to_string(generated_workflow_path("ci.yml")).expect("generated CI workflow");
    assert!(ci.contains("draft_release:"));
    assert!(
        !ci.contains("\n  build_release:\n"),
        "main-push CI must not publish release assets; release.yml owns that lifecycle"
    );
    assert!(
        !ci.contains("softprops/action-gh-release"),
        "main-push CI must not invoke the release asset publisher"
    );

    let bounty = std::fs::read_to_string(generated_workflow_path("bounty.yml"))
        .expect("generated bounty workflow");
    assert!(bounty.contains(
        "if: github.event_name == 'pull_request' || github.event_name == 'pull_request_target'",
    ));
}

#[test]
fn generate() {
    workflow::generate_ci_workflow();
}

#[test]
fn test_release_drafter() {
    let expected = std::fs::read_to_string(generated_workflow_path("release-drafter.yml"))
        .expect("release drafter workflow baseline");
    workflow::generate_release_drafter_workflow();

    let actual = std::fs::read_to_string(generated_workflow_path("release-drafter.yml"))
        .expect("release drafter workflow output");
    assert!(
        !actual.contains("Auto Labeler"),
        "pull_request_target must not execute label writes"
    );
    assert!(actual.contains("contents: write"));
    assert!(actual.contains("pull-requests: read"));
    assert!(
        actual.contains("release-drafter/release-drafter@5a60cd8ddda6dc14fce77159675b8fd2cdca4007")
    );

    let expected = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&expected).unwrap();
    let actual = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&actual).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn test_release_workflow() {
    let expected = include_str!("../../../.github/workflows/release.yml");
    let generated = workflow::release_publish_yaml().unwrap();
    assert!(!generated.contains("npm_release"));
    assert!(!generated.contains("homebrew_release"));
    assert!(generated.contains("Generate SHA-256 checksum"));
    assert!(generated.contains("shell: bash"));
    assert!(generated.contains("target: x86_64-unknown-linux-gnu"));
    assert!(generated.contains("target: x86_64-pc-windows-msvc"));
    assert!(generated.contains("matrix.binary_name }}.sha256"));
    assert!(generated.contains("attest_release_assets:"));
    assert!(generated.contains("needs: build_release"));
    assert!(generated.contains("attestations: write"));
    assert!(generated.contains("id-token: write"));
    assert!(generated.contains("Stage unsigned release assets"));
    assert!(generated.contains("release-assets-unsigned-${{ matrix.target }}"));
    assert!(generated.contains("publish_release_assets:"));
    assert!(generated.contains("needs: attest_release_assets"));
    assert!(generated.contains("gh release upload \"$RELEASE_TAG\" release-assets/*"));
    assert!(!generated.contains("upload-to-github-release"));
    assert!(generated.contains("helioslite_name: helioslite-x86_64-unknown-linux-musl"));
    assert!(generated.contains("helioslite_name: helioslite-x86_64-pc-windows-msvc.exe"));
    assert!(generated.contains("Generate helioslite SHA-256 checksum"));
    assert!(!generated.contains("Upload helioslite to Release"));
    assert!(!generated.contains("Upload helioslite checksum to Release"));
    assert!(
        !generated.contains(": \n"),
        "release workflow must not contain trailing whitespace"
    );
    assert!(
        !generated.contains("\\\\\n"),
        "shell continuations must have exactly one trailing backslash"
    );
    assert!(generated.contains("actions/attest-build-provenance@"));
    assert!(generated.contains("anchore/sbom-action@"));
    assert!(generated.contains("format: cyclonedx-json"));
    assert!(generated.contains("upload-release-assets: 'false'"));
    assert!(generated.contains("path: release-assets"));

    let expected = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(expected).unwrap();
    let actual = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&generated).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn test_labels_workflow() {
    let expected = std::fs::read_to_string(generated_workflow_path("labels.yml"))
        .expect("labels workflow baseline");
    workflow::generate_labels_workflow();
    let actual = std::fs::read_to_string(generated_workflow_path("labels.yml"))
        .expect("labels workflow output");

    let expected = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&expected).unwrap();
    let actual = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&actual).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn test_stale_workflow() {
    let expected = std::fs::read_to_string(generated_workflow_path("stale.yml"))
        .expect("stale workflow baseline");
    workflow::generate_stale_workflow();
    let actual = std::fs::read_to_string(generated_workflow_path("stale.yml"))
        .expect("stale workflow output");

    assert!(actual.contains("cron: 0 * * * *"));
    assert!(actual.contains("issues: write"));
    assert!(actual.contains("pull-requests: write"));
    assert!(actual.contains("actions/stale@1e223db275d687790206a7acac4d1a11bd6fe629"));

    let expected = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&expected).unwrap();
    let actual = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&actual).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn test_autofix_workflow() {
    let expected = std::fs::read_to_string(generated_workflow_path("autofix.yml"))
        .expect("autofix workflow baseline");
    workflow::generate_autofix_workflow();
    let actual = std::fs::read_to_string(generated_workflow_path("autofix.yml"))
        .expect("autofix workflow output");

    assert!(actual.contains("cancel-in-progress: false"));
    assert!(actual.contains("contents: read"));
    assert!(actual.contains("actions/checkout@d23441a48e516b6c34aea4fa41551a30e30af803"));

    let expected = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&expected).unwrap();
    let actual = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&actual).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn test_bounty_workflow() {
    let expected = std::fs::read_to_string(generated_workflow_path("bounty.yml"))
        .expect("bounty workflow baseline");
    workflow::generate_bounty_workflow();

    let actual = std::fs::read_to_string(generated_workflow_path("bounty.yml"))
        .expect("bounty workflow output");
    assert!(actual.contains(
        "if: github.event_name == 'pull_request' || github.event_name == 'pull_request_target'",
    ));
    assert!(actual.contains("issues: write"));
    assert!(actual.contains("pull-requests: write"));
    assert!(actual.contains("sync-all-issues.ts"));

    let expected = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&expected).unwrap();
    let actual = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&actual).unwrap();
    assert_eq!(actual, expected);
}
