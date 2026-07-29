#!/bin/bash
# Install script for luabox.
# Usage: curl -fsSL https://raw.githubusercontent.com/flying-dice/luabox/main/scripts/install.sh | bash
#
# Environment variables:
#   LUABOX_INSTALL_DIR   — where to install (default: ~/.luabox/bin)
#   LUABOX_VERSION       — version tag to install (default: latest)
#   LUABOX_DRAFT_INSTALL — CI only; set to 1 to install from a draft release
#                          (needs GITHUB_TOKEN and a pinned LUABOX_VERSION)
#   GITHUB_TOKEN         — CI only; see "draft-release path" below

set -euo pipefail

REPO="flying-dice/luabox"
BINARY="luabox"
INSTALL_DIR="${LUABOX_INSTALL_DIR:-$HOME/.luabox/bin}"
VERSION="${LUABOX_VERSION:-latest}"

# ── draft-release path ───────────────────────────────────────────────────────
# A GitHub *draft* release has no public release-download URLs, so the ordinary
# path below cannot see one. The release pipeline needs exactly that: it must
# install and fully exercise a release BEFORE publishing it
# (.github/workflows/release.yml → the `verify` job). So under an EXPLICIT
# LUABOX_DRAFT_INSTALL=1 opt-in — never on the mere presence of a token, which
# many CI environments export ambiently — assets are fetched through the
# authenticated GitHub API by asset id instead, which does see drafts.
#
# With LUABOX_DRAFT_INSTALL unset — every real user, every `curl … | bash` —
# none of this is reachable and the install is byte-for-byte what it always was.
TOKEN="${GITHUB_TOKEN:-}"
DRAFT_INSTALL="${LUABOX_DRAFT_INSTALL:-}"
if [ "$DRAFT_INSTALL" = "1" ]; then
    # Fail loudly on a half-configured opt-in: without a token the API cannot
    # see the draft, and a draft is never "latest" — silently falling back to
    # the public path would just 404 with a misleading message later.
    if [ -z "$TOKEN" ]; then
        echo "error: LUABOX_DRAFT_INSTALL=1 needs GITHUB_TOKEN set" >&2
        exit 1
    fi
    if [ "$VERSION" = "latest" ]; then
        echo "error: LUABOX_DRAFT_INSTALL=1 needs a pinned LUABOX_VERSION (a draft is never 'latest')" >&2
        exit 1
    fi
fi

detect_platform() {
    local os arch target

    os="$(uname -s)"
    arch="$(uname -m)"

    case "$os" in
        Linux)  os="unknown-linux-gnu" ;;
        Darwin) os="apple-darwin" ;;
        *)
            echo "error: unsupported OS: $os" >&2
            echo "       use the PowerShell script on Windows" >&2
            exit 1
            ;;
    esac

    case "$arch" in
        x86_64|amd64)   arch="x86_64" ;;
        aarch64|arm64)   arch="aarch64" ;;
        *)
            echo "error: unsupported architecture: $arch" >&2
            exit 1
            ;;
    esac

    target="${arch}-${os}"

    # Only targets with a prebuilt release asset are supported.
    case "$target" in
        x86_64-unknown-linux-gnu|aarch64-apple-darwin) ;;
        x86_64-apple-darwin)
            echo "error: no prebuilt binary for Intel macOS" >&2
            echo "       build from source: cargo install --git https://github.com/$REPO luabox-cli" >&2
            exit 1
            ;;
        aarch64-unknown-linux-gnu)
            echo "error: no prebuilt binary for aarch64 Linux" >&2
            echo "       build from source: cargo install --git https://github.com/$REPO luabox-cli" >&2
            exit 1
            ;;
        *)
            echo "error: no prebuilt binary for $target" >&2
            echo "       build from source: cargo install --git https://github.com/$REPO luabox-cli" >&2
            exit 1
            ;;
    esac

    echo "$target"
}

resolve_version() {
    if [ "$VERSION" = "latest" ]; then
        VERSION="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
            | grep '"tag_name"' \
            | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')"
        if [ -z "$VERSION" ]; then
            echo "error: could not resolve latest version" >&2
            echo "       are there any releases for $REPO yet?" >&2
            exit 1
        fi
    fi
    echo "$VERSION"
}

# Download a URL to a file, exiting with an optional hint on failure.
download_file() {
    local url="$1" dest="$2" hint="${3:-}"
    if ! curl -fsSL "$url" -o "$dest"; then
        echo "error: download failed: $url" >&2
        [ -n "$hint" ] && echo "       $hint" >&2
        exit 1
    fi
}

# True when assets must come from the authenticated API rather than the public
# release-download URLs: the explicit LUABOX_DRAFT_INSTALL=1 opt-in (its token
# and pinned-tag preconditions were enforced at startup).
use_api_downloads() {
    [ "$DRAFT_INSTALL" = "1" ]
}

# Print the numeric id of asset $2 on the release tagged $1, or return 1.
#
# Deliberately the LIST endpoint: `GET /releases/tags/<tag>` does not return a
# draft even with a token, so a draft is only reachable by listing releases and
# matching tag_name here. per_page=100 puts any realistic tag on page 1; the
# loop walks on regardless rather than assuming it.
find_asset_id() {
    local tag="$1" name="$2" page=1 json id
    while [ "$page" -le 5 ]; do
        if ! json="$(curl -fsSL \
            -H "Accept: application/vnd.github+json" \
            -H "Authorization: Bearer $TOKEN" \
            "https://api.github.com/repos/$REPO/releases?per_page=100&page=$page")"; then
            return 1
        fi
        id="$(printf '%s' "$json" | jq -r --arg tag "$tag" --arg name "$name" '
            (map(select(.tag_name == $tag))[0].assets // [])
            | map(select(.name == $name))[0].id // empty
        ')"
        if [ -n "$id" ]; then
            printf '%s' "$id"
            return 0
        fi
        # An empty page means the listing is exhausted; nothing left to walk.
        [ "$(printf '%s' "$json" | jq -r 'length')" -gt 0 ] || return 1
        page=$((page + 1))
    done
    return 1
}

# Download release asset $2 of release $1 to $3 through the authenticated API.
#
# `curl -L` is what makes this safe: the asset endpoint 302s to a pre-signed
# storage URL, and curl drops the Authorization header across hosts (only
# --location-trusted would forward it), so the storage side never sees two
# competing auth mechanisms.
download_asset_api() {
    local tag="$1" name="$2" dest="$3" id
    if ! command -v jq >/dev/null 2>&1; then
        echo "error: installing from a draft release needs 'jq'" >&2
        echo "       unset LUABOX_DRAFT_INSTALL to use the public release-download path" >&2
        exit 1
    fi
    if ! id="$(find_asset_id "$tag" "$name")"; then
        echo "error: no asset '$name' on release '$tag'" >&2
        echo "       is GITHUB_TOKEN allowed to read releases of $REPO?" >&2
        exit 1
    fi
    if ! curl -fsSL \
        -H "Accept: application/octet-stream" \
        -H "Authorization: Bearer $TOKEN" \
        "https://api.github.com/repos/$REPO/releases/assets/$id" -o "$dest"; then
        echo "error: download failed: asset '$name' (id $id) of release '$tag'" >&2
        exit 1
    fi
}

# Fetch release asset NAME of release VER to DEST, by whichever of the two
# paths applies. HINT is the public path's failure hint — the only path a user
# ever takes, and the only one whose wording is user-facing.
fetch_asset() {
    local ver="$1" name="$2" dest="$3" hint="${4:-}"
    if use_api_downloads; then
        download_asset_api "$ver" "$name" "$dest"
    else
        download_file "https://github.com/$REPO/releases/download/${ver}/${name}" "$dest" "$hint"
    fi
}

# Verify a downloaded artifact against the release's SHA256SUMS asset.
verify_checksum() {
    local file="$1" name="$2" ver="$3" dir="$4" expected actual

    echo "  Verifying checksum ..."
    fetch_asset "$ver" "SHA256SUMS" "$dir/SHA256SUMS" "is there a release $ver?"

    expected="$(awk -v f="$name" '$2 == f { print $1 }' "$dir/SHA256SUMS")"
    if [ -z "$expected" ]; then
        echo "error: no checksum listed for $name" >&2
        exit 1
    fi

    if command -v sha256sum >/dev/null 2>&1; then
        actual="$(sha256sum "$file" | awk '{ print $1 }')"
    elif command -v shasum >/dev/null 2>&1; then
        actual="$(shasum -a 256 "$file" | awk '{ print $1 }')"
    else
        echo "error: no SHA-256 tool found (need sha256sum or shasum)" >&2
        exit 1
    fi

    if [ "$expected" != "$actual" ]; then
        echo "error: checksum mismatch for $name" >&2
        echo "       expected: $expected" >&2
        echo "       actual:   $actual" >&2
        exit 1
    fi
}

# Print PATH setup guidance, unless INSTALL_DIR is already on PATH.
print_path_hint() {
    case ":$PATH:" in
        *":$INSTALL_DIR:"*) return ;;
    esac

    local shell_name
    shell_name="$(basename "${SHELL:-}")"
    echo ""
    echo "Add to your PATH:"
    echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
    echo ""
    echo "Or add to your shell profile:"
    case "$shell_name" in
        zsh)  echo "  echo 'export PATH=\"$INSTALL_DIR:\$PATH\"' >> ~/.zshrc" ;;
        bash) echo "  echo 'export PATH=\"$INSTALL_DIR:\$PATH\"' >> ~/.bashrc" ;;
        fish) echo "  fish_add_path $INSTALL_DIR" ;;
        *)    echo "  echo 'export PATH=\"$INSTALL_DIR:\$PATH\"' >> ~/.\${SHELL}rc" ;;
    esac
}

# Temp dir is script-global, not `local` to main: the EXIT trap below runs after
# main returns, where a function-local would be out of scope and `set -u` would
# abort the trap (unbound variable) — leaking the dir and exiting non-zero.
tmp_dir=""

main() {
    local target version artifact_name archive_name download_url

    echo "Installing luabox..."

    target="$(detect_platform)"
    version="$(resolve_version)"
    artifact_name="luabox-${target}"
    archive_name="${artifact_name}.tar.gz"

    echo "  Platform: $target"
    echo "  Version:  $version"
    echo "  Install:  $INSTALL_DIR"

    download_url="https://github.com/$REPO/releases/download/${version}/${archive_name}"

    tmp_dir="$(mktemp -d)"
    trap 'rm -rf "${tmp_dir:-}"' EXIT

    if use_api_downloads; then
        echo "  Downloading $archive_name via the GitHub release API (draft-aware) ..."
    else
        echo "  Downloading $download_url ..."
    fi
    fetch_asset "$version" "$archive_name" "$tmp_dir/archive.tar.gz" \
        "check that version '$version' has a release asset for '$target'"

    verify_checksum "$tmp_dir/archive.tar.gz" "$archive_name" "$version" "$tmp_dir"

    tar -xzf "$tmp_dir/archive.tar.gz" -C "$tmp_dir"

    mkdir -p "$INSTALL_DIR"
    mv "$tmp_dir/$BINARY" "$INSTALL_DIR/$BINARY"
    chmod +x "$INSTALL_DIR/$BINARY"

    echo ""
    echo "luabox $version installed to $INSTALL_DIR/$BINARY"

    print_path_hint
}

main
