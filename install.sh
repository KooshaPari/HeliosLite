#!/usr/bin/env bash
# install.sh — Install HeliosLite (formerly Forgecode) on POSIX systems
#
# Usage:
#   curl -fsSL https://helioslite.phenotype.space/install.sh | bash
#
#   # Pin a specific version:
#   curl -fsSL https://helioslite.phenotype.space/install.sh | bash -s -- 1.2.3
#
#   # Local install (no download): run from repo root
#   ./install.sh --local
#
#   # Override automatic Linux GNU/musl detection (useful in CI):
#   HELIOSLITE_TARGET=x86_64-unknown-linux-musl ./install.sh
#
# Installs `helioslite` on PATH, the legacy `forge` alias, and — when the
# release carries it — the `forge_dbd` storage daemon. On Linux/macOS the
# release publishes one binary under two asset names; `forge-<target>` is the
# asset fetched here and it is installed under both command names. Every
# binary is verified against the published `.sha256` sidecar before it
# replaces anything on disk.

set -euo pipefail

VERSION=""
LOCAL=0
REPO="${HELIOSLITE_RELEASE_REPO:-KooshaPari/forgecode}"
TARGET_OVERRIDE="${HELIOSLITE_TARGET:-}"

validate_repo() { [[ "$1" =~ ^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$ ]] || { echo "Invalid release repo: $1" >&2; exit 1; }; }
validate_version() { printf '%s' "$1" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$' || { echo "Invalid release version: $1" >&2; exit 1; }; }
validate_reported_version() {
    local output="$1"
    printf '%s\n' "$output" | grep -Eq '(^|[[:space:]])v?[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?([[:space:]]|$)' \
        || { echo "Installed binary did not report a semantic version" >&2; exit 1; }
}
validate_repo "$REPO"

for arg in "$@"; do
    case "$arg" in
        --local)             LOCAL=1 ;;
        --skip-forge)        : ;; # Deprecated compatibility no-op; only helioslite is installed.
        --skip-update-check) : ;; # Deprecated compatibility no-op; the binary checks for updates itself.
        --help|-h)
            sed -n '2,10p' "$0"
            exit 0
            ;;
        -*) echo "Unknown flag: $arg" >&2; exit 1 ;;
        *)  VERSION="$arg" ;;
    esac
done

# Accept either `1.2.3` or the GitHub-style `v1.2.3` spelling.
VERSION="${VERSION#v}"

# 1) Resolve target version
if [ -z "$VERSION" ] && [ "$LOCAL" = "0" ]; then
    VERSION="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
                | grep -oE '"tag_name":\s*"v?[0-9][^"]*"' \
                | head -1 \
                | sed -E 's/.*"v?([^"]+)".*/\1/' || true)"
    if [ -z "$VERSION" ]; then
        echo -e "  ✖ \033[31mCould not determine latest version; refusing an unpinned install\033[0m" >&2
        exit 1
    fi
fi
if [ "$LOCAL" = "0" ]; then
    validate_version "$VERSION"
    echo -e "  → \033[36mTarget version: $VERSION\033[0m"
else
    echo -e "  → \033[36mTarget version: local build\033[0m"
fi

# 2) Pick install location
INSTALL_DIR="${HELIOSLITE_INSTALL_DIR:-$HOME/.helioslite/bin}"
mkdir -p "$INSTALL_DIR"

# 3) Detect target triple
detect_target() {
    local os arch libc
    os="$(uname -s | tr '[:upper:]' '[:lower:]')"
    arch="$(uname -m)"
    case "$arch" in
        x86_64|amd64) arch="x86_64" ;;
        aarch64|arm64) arch="aarch64" ;;
        *) echo -e "  ✖ \033[31mUnsupported architecture: $arch\033[0m"; return 1 ;;
    esac
    case "$os" in
        linux)
            # Allow CI/packagers to pin an exact release target, but never
            # interpolate arbitrary input into an asset URL.
            if [ -n "$TARGET_OVERRIDE" ]; then
                case "$TARGET_OVERRIDE" in
                    x86_64-unknown-linux-gnu|x86_64-unknown-linux-musl|\
                    aarch64-unknown-linux-gnu|aarch64-unknown-linux-musl)
                        echo "$TARGET_OVERRIDE"; return 0 ;;
                    *)
                        echo -e "  ✖ \033[31mUnsupported HELIOSLITE_TARGET: $TARGET_OVERRIDE\033[0m" >&2
                        return 1 ;;
                esac
            fi
            libc="gnu"
            # musl's ldd identifies itself in its version output.  The
            # loader check covers minimal Alpine images where ldd is absent.
            if { command -v ldd >/dev/null 2>&1 && ldd --version 2>&1 \
                    | grep -qi musl; } \
                || compgen -G '/lib/ld-musl-*.so.1' >/dev/null 2>&1 \
                || compgen -G '/lib64/ld-musl-*.so.1' >/dev/null 2>&1; then
                libc="musl"
            fi
            echo "${arch}-unknown-linux-${libc}"
            ;;
        darwin)  echo "${arch}-apple-darwin" ;;
        *) echo -e "  ✖ \033[31mUnsupported OS: $os\033[0m"; return 1 ;;
    esac
}

# Fetch one release asset into $INSTALL_DIR, verifying its published digest.
#
#   fetch_asset <asset-name> <destination-filename> <required: 1|0>
#
# The verified bytes are staged beside the destination and then moved over it,
# so a failed or interrupted run leaves the previously installed binary intact
# rather than a truncated one. Optional assets skip with a warning; required
# assets exit non-zero, because a partial install that silently omits the
# canonical binary is worse than a loud failure.
fetch_asset() {
    local asset="$1" dest="$2" required="$3"
    local url="https://github.com/$REPO/releases/download/v$VERSION/$asset"
    local staged="$TMP/$asset" expected actual incoming

    # These use explicit `if !` blocks rather than `cmd || handler`. Under
    # `set -e` the final command of an `||` list is not protected, so a
    # handler that returned non-zero for an optional asset would abort the
    # whole install instead of skipping it.
    if ! curl -fsSL "$url" -o "$staged"; then
        if [ "$required" = "1" ]; then
            echo -e "  ✖ \033[31mDownload failed: $asset\033[0m" >&2
            exit 1
        fi
        echo -e "  ! \033[33mSkipping optional asset $asset — download failed\033[0m" >&2
        return 1
    fi
    if ! curl -fsSL "$url.sha256" -o "$staged.sha256"; then
        if [ "$required" = "1" ]; then
            echo -e "  ✖ \033[31mRelease checksum is unavailable; refusing an unverified binary\033[0m" >&2
            exit 1
        fi
        echo -e "  ! \033[33mSkipping optional asset $asset — no published checksum\033[0m" >&2
        return 1
    fi

    expected="$(awk 'NF { print $1; exit }' "$staged.sha256")"
    case "$expected" in
        (''|*[!0123456789abcdefABCDEF]*)
            echo -e "  ✖ \033[31mInvalid SHA-256 checksum format for $asset\033[0m" >&2
            exit 1
            ;;
    esac
    if [ "${#expected}" -ne 64 ]; then
        echo -e "  ✖ \033[31mInvalid SHA-256 checksum length for $asset\033[0m" >&2
        exit 1
    fi

    if command -v sha256sum >/dev/null 2>&1; then
        actual="$(sha256sum "$staged" | awk '{print $1}')"
    elif command -v shasum >/dev/null 2>&1; then
        actual="$(shasum -a 256 "$staged" | awk '{print $1}')"
    else
        echo -e "  ✖ \033[31mNo SHA-256 utility found; refusing an unverified binary\033[0m" >&2
        exit 1
    fi

    if [ "$(printf '%s' "$expected" | tr '[:upper:]' '[:lower:]')" \
        != "$(printf '%s' "$actual" | tr '[:upper:]' '[:lower:]')" ]; then
        echo -e "  ✖ \033[31mSHA-256 verification failed for $asset\033[0m" >&2
        exit 1
    fi

    incoming="$INSTALL_DIR/.$dest.tmp.$$"
    cp "$staged" "$incoming"
    chmod +x "$incoming"
    mv -f "$incoming" "$INSTALL_DIR/$dest"
    echo -e "  ✓ \033[32m$asset — SHA-256 verified\033[0m"
    return 0
}

# Link a command name to an already-installed sibling binary. A symlink keeps
# the alias and the canonical name from drifting apart across upgrades; a copy
# is used only if the filesystem refuses symlinks.
link_alias() {
    local alias="$1" target="$2"
    [ "$alias" = "$target" ] && return 0
    ln -sfn "$target" "$INSTALL_DIR/$alias" 2>/dev/null \
        || cp -f "$INSTALL_DIR/$target" "$INSTALL_DIR/$alias"
    chmod +x "$INSTALL_DIR/$alias"
}

if [ "$LOCAL" = "1" ]; then
    if ! command -v cargo >/dev/null 2>&1; then
        echo -e "  ✖ \033[31mcargo not on PATH — install rustup: https://rustup.rs/\033[0m"
        exit 1
    fi
    echo -e "  → \033[36mLocal install — building from source...\033[0m"
    pushd "$(cd "$(dirname "$0")" && pwd)" >/dev/null
    cargo build --release --bin helioslite --bin forge --bin forge_dbd
    install -m 0755 "target/release/helioslite" "$INSTALL_DIR/helioslite"
    link_alias "forge" "helioslite"
    if [ -f "target/release/forge_dbd" ]; then
        install -m 0755 "target/release/forge_dbd" "$INSTALL_DIR/forge_dbd"
    else
        echo -e "  ! \033[33mforge_dbd was not built; skipping\033[0m" >&2
    fi
    popd >/dev/null
else
    TARGET="$(detect_target)"
    TMP="$(mktemp -d -t helioslite-install-XXXXXX)"
    trap 'rm -rf "$TMP"' EXIT INT TERM

    fetch_asset "forge-${TARGET}" "helioslite" 1
    link_alias "forge" "helioslite"
    fetch_asset "forge_dbd-${TARGET}" "forge_dbd" 0 || true

    trap - EXIT INT TERM
    rm -rf "$TMP"
fi
chmod +x "$INSTALL_DIR/helioslite"

# 4) PATH
add_to_path() {
    local dir="$1"
    case ":$PATH:" in
        *":$dir:"*) return 0 ;;
    esac
    for rc in "$HOME/.bashrc" "$HOME/.zshrc" "$HOME/.profile"; do
        if [ -f "$rc" ]; then
            if ! grep -q "$dir" "$rc"; then
                echo "" >> "$rc"
                echo "# Added by helioslite installer" >> "$rc"
                echo "export PATH=\"\$PATH:$dir\"" >> "$rc"
            fi
        fi
    done
    export PATH="$PATH:$dir"
}
add_to_path "$INSTALL_DIR"

# 5) Verify every command that was installed actually runs and reports the
#    requested version. A binary that exists but cannot execute is not a
#    successful install.
if [ "$LOCAL" = "0" ]; then
    EXPECTED_VERSION_PATTERN="${VERSION//./\\.}"
else
    EXPECTED_VERSION_PATTERN="[0-9]+\\.[0-9]+\\.[0-9]+"
fi

#   verify_command <command> <strict: 1|0>
#
# `helioslite` and `forge` are the same binary and must report exactly the
# requested release version. `forge_dbd` lives in its own crate and is
# versioned independently of the workspace, so it is only required to be
# executable and to report some semantic version — demanding the release tag
# from it would reject every legitimate build.
verify_command() {
    local name="$1" strict="$2" output
    if [ ! -x "$INSTALL_DIR/$name" ]; then
        echo -e "  ✖ \033[31m$name was not installed\033[0m" >&2
        exit 1
    fi
    output="$("$INSTALL_DIR/$name" --version 2>&1 | head -n 1 || true)"
    if [ -z "$output" ]; then
        echo -e "  ✖ \033[31m$name --version returned no output; refusing an unverified install\033[0m" >&2
        exit 1
    fi
    validate_reported_version "$output"
    if [ "$strict" = "1" ] \
        && ! printf '%s\n' "$output" | grep -Eq "(^|[[:space:]])v?${EXPECTED_VERSION_PATTERN}([[:space:]]|$)"; then
        echo -e "  ✖ \033[31m$name reports a version that does not match the requested ${VERSION:-build}\033[0m" >&2
        exit 1
    fi
    echo -e "  ✓ \033[32m$name reports: $output\033[0m"
}

verify_command "helioslite" 1
verify_command "forge" 1
# forge_dbd is optional, so it is verified only when it is present.
if [ -x "$INSTALL_DIR/forge_dbd" ]; then
    verify_command "forge_dbd" 0
fi

echo ""
echo -e "  🎉 \033[32mHeliosLite installed.\033[0m"
echo -e "     Commands: helioslite (canonical), forge (legacy alias), forge_dbd (storage daemon)"
echo -e "     Try:  helioslite --help"
echo -e "     Docs: https://helioslite.phenotype.space"
