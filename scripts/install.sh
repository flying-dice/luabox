#!/bin/bash
# Install script for luabox.
# Usage: curl -fsSL https://raw.githubusercontent.com/flying-dice/luabox/main/scripts/install.sh | bash
#
# Environment variables:
#   LUABOX_INSTALL_DIR  — where to install (default: ~/.luabox/bin)
#   LUABOX_VERSION      — version tag to install (default: latest)

set -euo pipefail

REPO="flying-dice/luabox"
BINARY="luabox"
INSTALL_DIR="${LUABOX_INSTALL_DIR:-$HOME/.luabox/bin}"
VERSION="${LUABOX_VERSION:-latest}"

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

# Verify a downloaded artifact against the release's SHA256SUMS asset.
verify_checksum() {
    local file="$1" name="$2" ver="$3" dir="$4" sums_url expected actual

    sums_url="https://github.com/$REPO/releases/download/${ver}/SHA256SUMS"

    echo "  Verifying checksum ..."
    download_file "$sums_url" "$dir/SHA256SUMS" "is there a release $ver?"

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
    local target version artifact_name download_url

    echo "Installing luabox..."

    target="$(detect_platform)"
    version="$(resolve_version)"
    artifact_name="luabox-${target}"

    echo "  Platform: $target"
    echo "  Version:  $version"
    echo "  Install:  $INSTALL_DIR"

    download_url="https://github.com/$REPO/releases/download/${version}/${artifact_name}.tar.gz"

    tmp_dir="$(mktemp -d)"
    trap 'rm -rf "${tmp_dir:-}"' EXIT

    echo "  Downloading $download_url ..."
    download_file "$download_url" "$tmp_dir/archive.tar.gz" \
        "check that version '$version' has a release asset for '$target'"

    verify_checksum "$tmp_dir/archive.tar.gz" "${artifact_name}.tar.gz" "$version" "$tmp_dir"

    tar -xzf "$tmp_dir/archive.tar.gz" -C "$tmp_dir"

    mkdir -p "$INSTALL_DIR"
    mv "$tmp_dir/$BINARY" "$INSTALL_DIR/$BINARY"
    chmod +x "$INSTALL_DIR/$BINARY"

    echo ""
    echo "luabox $version installed to $INSTALL_DIR/$BINARY"

    print_path_hint
}

main
