use crate::jobs::ReleaseBuilderJob;
use crate::workflow_model::{Event, Job, Level, Permissions, Step, Workflow};

const ARTIFACT_DOWNLOAD: &str = "d3f86a106a0bac45b974a628896c90dbdf5c8093";
const ARTIFACT_UPLOAD: &str = "ea165f8d65b6e75b540449e92b4886f43607fa02";

/// Generate the release build and asset-attestation workflow.
///
/// Assets are never published by build jobs. Each target stages an artifact;
/// signing replaces macOS/Windows artifacts, SBOM and provenance consume that
/// staged set, and the final job is the sole release publisher.
pub fn release_publish() {
    super::generate_private_workflow(release_workflow(), "release.yml");
}

/// Render the release workflow without modifying the checked-in fixture.
pub fn release_publish_yaml() -> Result<String, serde_yaml_ng::Error> {
    release_workflow().to_yaml()
}

fn download_staged_assets() -> Step {
    Step::new("Download staged release assets")
        .uses("actions", "download-artifact", ARTIFACT_DOWNLOAD)
        .input("pattern", "release-assets-*")
        .input("path", "staged-assets")
}

fn assemble_signed_assets() -> Step {
    Step::new("Assemble signed release assets").run(
        "set -euo pipefail\nmkdir -p release-assets\nfor asset_dir in staged-assets/*; do\n  case \"$asset_dir\" in\n    *unsigned-*-apple-darwin|*unsigned-*-windows-msvc) continue ;;\n  esac\n  find \"$asset_dir\" -maxdepth 1 -type f -exec cp {} release-assets/ \\;\ndone",
    )
}

fn release_workflow() -> Workflow {
    let release_build_job =
        ReleaseBuilderJob::new("${{ github.event.release.tag_name }}").stage_assets(true);
    let sbom_job = Job::new("Generate release SBOM")
        .needs("sign_release")
        .permissions(Permissions::default().contents(Level::Read))
        .add_step(download_staged_assets())
        .add_step(assemble_signed_assets())
        .add_step(
            Step::new("Generate CycloneDX SBOM")
                .uses(
                    "anchore",
                    "sbom-action",
                    "6b92ff5b2cce1787a99198f282dd8a26d1991449",
                )
                .input("path", "release-assets")
                .input("format", "cyclonedx-json")
                .input(
                    "artifact-name",
                    "forgecode-${{ github.event.release.tag_name }}.cdx.json",
                )
                .input("output-file", "release-assets/sbom.cdx.json")
                .input("upload-artifact", "false")
                .input("upload-release-assets", "false"),
        )
        .add_step(
            Step::new("Stage release SBOM")
                .uses("actions", "upload-artifact", ARTIFACT_UPLOAD)
                .input("name", "release-sbom")
                .input("path", "release-assets/sbom.cdx.json")
                .input("if-no-files-found", "error"),
        );
    let attest_job = Job::new("Attest release assets")
        .needs("sbom_release_assets")
        .permissions(
            Permissions::default()
                .contents(Level::Read)
                .id_token(Level::Write)
                .attestations(Level::Write),
        )
        .add_step(download_staged_assets())
        .add_step(assemble_signed_assets())
        .add_step(
            Step::new("Download release SBOM")
                .uses("actions", "download-artifact", ARTIFACT_DOWNLOAD)
                .input("name", "release-sbom")
                .input("path", "release-assets"),
        )
        .add_step(
            Step::new("Attest release assets")
                .uses(
                    "actions",
                    "attest-build-provenance",
                    "0f67c3f4856b2e3261c31976d6725780e5e4c373",
                )
                .input("subject-path", "release-assets/*"),
        );
    let publish_job = Job::new("Publish verified release assets")
        .needs("attest_release_assets")
        .permissions(Permissions::default().contents(Level::Write))
        .add_step(download_staged_assets())
        .add_step(assemble_signed_assets())
        .add_step(
            Step::new("Download release SBOM")
                .uses("actions", "download-artifact", ARTIFACT_DOWNLOAD)
                .input("name", "release-sbom")
                .input("path", "release-assets"),
        )
        .add_step(
            Step::new("Publish verified release assets")
                .env("GH_TOKEN", "${{ github.token }}")
                .env("RELEASE_TAG", "${{ github.event.release.tag_name }}")
                .run("set -euo pipefail\ngh release upload \"$RELEASE_TAG\" release-assets/* --repo \"${{ github.repository }}\" --clobber"),
        );
    Workflow::new("Multi Channel Release")
        .on(Event::default().release(["published"]))
        .permissions(Permissions::default().contents(Level::Read))
        .add_job("build_release", release_build_job.into_job())
        .add_job(
            "sign_release",
            Job::reusable(
                "Sign release and regenerate checksums",
                "./.github/workflows/sign-release.yml",
            )
            .input("tag", "${{ github.event.release.tag_name }}")
            .secret("MACOS_CERTIFICATE")
            .secret("MACOS_CERTIFICATE_PWD")
            .secret("MACOS_SIGNING_IDENTITY")
            .secret("MACOS_NOTARIZATION_APPLE_ID")
            .secret("MACOS_NOTARIZATION_PWD")
            .secret("MACOS_NOTARIZATION_TEAM_ID")
            .secret("MACOS_NOTARIZATION_API_KEY")
            .secret("MACOS_NOTARIZATION_KEY_ID")
            .secret("MACOS_NOTARIZATION_ISSUER_ID")
            .secret("SIGNPATH_API_TOKEN")
            .secret("SIGNPATH_ORGANIZATION_ID")
            .needs("build_release")
            // The reusable sign job (sign-release.yml) writes back signed
            // assets, so the caller must grant contents:write: reusable
            // workflows cannot elevate permissions beyond the caller grant.
            .permissions(
                Permissions::default()
                    .contents(Level::Write)
                    .actions(Level::Read),
            ),
        )
        .add_job("sbom_release_assets", sbom_job)
        .add_job("attest_release_assets", attest_job)
        .add_job("publish_release_assets", publish_job)
}
