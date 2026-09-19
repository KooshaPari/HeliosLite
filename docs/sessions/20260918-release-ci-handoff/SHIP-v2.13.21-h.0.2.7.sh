#!/usr/bin/env bash
# Ship HeliosLite v2.13.21-h.0.2.7 — the release that carries the language-server fixes.
#
# Why this exists: the fixes are on main, but the newest published release
# (v2.13.21-h.0.2.6) predates them, and publishing needs an authenticated API call.
# This host's gh token was invalid at handoff time, so the final step is scripted
# instead of left as prose.
#
# Prerequisite: an authenticated gh.  Run `gh auth login -h github.com` first
# (device flow; approve in a browser).
#
# Expect ~55 assets afterwards: 27 binaries + 27 .sha256 + sbom.cdx.json.
# Verify them the way section 4a of HANDOFF.md describes: download anonymously,
# compare each .sha256, execute the macOS binaries, `codesign --verify --strict`.

set -euo pipefail

REPO="KooshaPari/HeliosLite"
TAG="v2.13.21-h.0.2.7"
SHA="aaef98bf0"

# repo root = three levels up from docs/sessions/<session>/
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
NOTES="$SCRIPT_DIR/RELEASE-NOTES-$TAG.md"

cd "$REPO_ROOT"

if ! gh auth status >/dev/null 2>&1; then
  echo "error: gh is not authenticated. Run: gh auth login -h github.com" >&2
  exit 1
fi

if [ ! -f "$NOTES" ]; then
  echo "error: release notes not found at $NOTES" >&2
  exit 1
fi

# Tag. Note: `release.yml` triggers on `release: published`, so the tag alone
# publishes nothing — always pair it with the release creation below.
if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
  echo "tag $TAG already exists locally; reusing it"
else
  git tag "$TAG" "$SHA"
  echo "created tag $TAG -> $SHA"
fi
git push fork "$TAG"

if gh release view "$TAG" --repo "$REPO" >/dev/null 2>&1; then
  echo "release $TAG already exists; nothing to create"
else
  gh release create "$TAG" --repo "$REPO" --title "$TAG" --notes-file "$NOTES"
  echo "created release $TAG"
fi

echo
echo "next: confirm the 'Multi Channel Release' run went green and attached ~55 assets"
echo "  gh run list --repo $REPO --workflow release.yml --limit 3"
echo "  gh api repos/$REPO/releases/tags/$TAG --jq '.assets | length'"
