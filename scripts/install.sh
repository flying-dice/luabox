#!/usr/bin/env bash
# Install the latest luabox release binary from the canonical GitLab
# instance (gitlab.beluga-sirius.ts.net:flying-dice/luabox) — Linux/macOS.
# Windows counterpart: scripts/install.ps1.
#
# Usage:
#   curl -fsSL https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/raw/main/scripts/install.sh | bash
#   # or, checked out locally:
#   bash scripts/install.sh
#
# Env overrides:
#   LUABOX_INSTALL_DIR   where to place the binary (default: ~/.local/bin)
#
# Until the first `v*` tag exists (see RELEASING.md), the GitLab releases
# API has nothing to serve; this script detects that and errors out with a
# pointer to the `cargo install --git` fallback below. That means this
# script cannot be end-to-end verified until the first tag lands — it's
# written defensively (every external command's exit status checked) but
# the "happy path" download-and-verify is unproven in CI today.
set -eu

GITLAB_HOST="gitlab.beluga-sirius.ts.net"
PROJECT_PATH="flying-dice/luabox"
PROJECT_PATH_ENCODED="flying-dice%2Fluabox"
API_BASE="https://${GITLAB_HOST}/api/v4/projects/${PROJECT_PATH_ENCODED}"
INSTALL_DIR="${LUABOX_INSTALL_DIR:-$HOME/.local/bin}"

log() { printf '==> %s\n' "$*"; }
err() { printf 'error: %s\n' "$*" >&2; }
warn() { printf 'warning: %s\n' "$*" >&2; }

fallback_hint() {
    cat >&2 <<EOF

No published release was found. Until the first v* tag lands (see
RELEASING.md), install straight from source instead:

    cargo install --git ssh://git@${GITLAB_HOST}/${PROJECT_PATH}.git luabox-cli

(requires an SSH key registered with the GitLab instance, and a Rust
toolchain — see https://rustup.rs).
EOF
}

need() {
    command -v "$1" >/dev/null 2>&1 || { err "'$1' is required but not found on PATH"; exit 1; }
}

need curl

os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
    Linux) platform="linux" ;;
    Darwin) platform="macos" ;;
    *)
        err "unsupported OS '${os}' — this script covers Linux and macOS only. On Windows, use scripts/install.ps1."
        exit 1
        ;;
esac
case "$arch" in
    x86_64 | amd64) ;;
    *)
        err "unsupported architecture '${arch}' — only x86_64 release binaries are published today."
        exit 1
        ;;
esac
asset_name="luabox-x86_64-${platform}"

log "looking up the latest release for ${PROJECT_PATH} on ${GITLAB_HOST}..."
release_json="$(curl -fsSL "${API_BASE}/releases/permalink/latest" 2>/dev/null || true)"

if [ -z "$release_json" ] || ! printf '%s' "$release_json" | grep -q '"tag_name"'; then
    err "no release found at ${API_BASE}/releases/permalink/latest"
    fallback_hint
    exit 1
fi

# Scrape the asset URL out of the release JSON without a JSON-parser
# dependency (curl + grep/sed is all this needs to promise): GitLab's
# release payload nests assets at .assets.links[] as {"name": ..., "url":
# ...} objects. Splitting on `}` isolates one object per line so a
# name-then-url grep pair can't cross into a neighboring asset.
asset_url="$(
    printf '%s' "$release_json" \
        | tr '}' '\n' \
        | grep "\"name\":[[:space:]]*\"${asset_name}\"" \
        | grep -o '"url"[[:space:]]*:[[:space:]]*"[^"]*"' \
        | head -n1 \
        | sed -E 's/.*"url"[[:space:]]*:[[:space:]]*"([^"]*)".*/\1/'
)"

if [ -z "$asset_url" ]; then
    tag="$(printf '%s' "$release_json" | grep -o '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' | head -n1 | sed -E 's/.*:[[:space:]]*"([^"]*)".*/\1/')"
    err "release ${tag:-<unknown>} has no '${asset_name}' asset"
    fallback_hint
    exit 1
fi

log "found asset: ${asset_url}"

tmp_bin="$(mktemp)"
cleanup() { rm -f "$tmp_bin"; }
trap cleanup EXIT

log "downloading..."
curl -fsSL "$asset_url" -o "$tmp_bin"
chmod +x "$tmp_bin"

mkdir -p "$INSTALL_DIR"
dest="${INSTALL_DIR}/luabox"
mv "$tmp_bin" "$dest"
trap - EXIT

log "installed to ${dest}"

case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *) warn "${INSTALL_DIR} is not on PATH; add it to your shell profile (e.g. export PATH=\"${INSTALL_DIR}:\$PATH\")" ;;
esac

log "verifying..."
if "$dest" --version; then
    log "luabox installed successfully."
else
    err "installed binary at ${dest} failed to run '--version' — the download may be corrupt or for the wrong platform"
    exit 1
fi
