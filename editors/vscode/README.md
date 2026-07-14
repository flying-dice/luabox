# luabox for VS Code

First-class VS Code support for the
[luabox](https://github.com/flying-dice/luabox) Lua
toolchain. This extension is a thin wrapper around the `luabox lsp`
language server: typecheck + lint diagnostics with quick-fixes, hover,
completion (incl. auto-require imports), goto definition/type-definition/
implementation, find-references, rename, symbols, signature help, call
hierarchy, inlay hints, formatting and semantic highlighting for `.lua`
sources.

## What it provides

- Launches `luabox lsp` (stdio) via `vscode-languageclient` and attaches it to
  `lua` documents.
- **Formatting** (`Format Document`, and `Format Selection` with
  whole-document MVP semantics) via the canonical luabox formatters — see
  [Formatting](#formatting) below.
- **Semantic highlighting**: the server advertises semantic tokens with only
  standard token types/modifiers, and `vscode-languageclient` requests them
  automatically — no theme configuration needed. Locals vs globals,
  parameters and LuaCATS `---@` annotation comments all render distinctly.
- A `luabox.path` setting to point at a specific `luabox` binary.
- A `luabox: Restart Language Server` command (`luabox.restartServer`).

> The extension attaches to `.lua` files **without** declaring the `lua`
> language itself, so it never conflicts with the grammar shipped by other Lua
> extensions (e.g. sumneko/LuaLS). Install a Lua grammar extension alongside it
> for `.lua` syntax highlighting.

## Requirements

- A `luabox` binary on your `PATH` (or configured via `luabox.path`). The
  normal way to get one is the install script — see [Getting the `luabox`
  binary](#getting-the-luabox-binary) below. Building from source
  (`cargo build --release`) is only needed if you're developing luabox
  itself.
- VS Code `^1.85.0`.

## Getting the `luabox` binary

From a released build, run the install script for your platform (see the
repo root [`RELEASING.md`](../../RELEASING.md) for how releases are cut):

```sh
# Linux / macOS
curl -fsSL https://raw.githubusercontent.com/flying-dice/luabox/main/scripts/install.sh | bash
```

```powershell
# Windows
irm https://raw.githubusercontent.com/flying-dice/luabox/main/scripts/install.ps1 | iex
```

Both scripts fetch the latest tagged GitHub Release's binary asset, verify it
against the release's `SHA256SUMS`, and install it to `~/.luabox/bin`
(Linux/macOS) or `%USERPROFILE%\.luabox\bin` (Windows) by default — override
with `LUABOX_INSTALL_DIR`, or pin a release with `LUABOX_VERSION=vX.Y.Z`.

Then install this extension itself — see [Installing the extension](#installing-the-extension).

## Installing the extension

- **From a release**: the packaged `.vsix` is attached to each GitHub release —
  download it (or build one yourself — see [Package a `.vsix`](#package-a-vsix))
  and run `code --install-extension luabox-<version>.vsix`, or use
  **Extensions ▸ … ▸ Install from VSIX** in the UI.
- **From the Marketplace / Open VSX**: not yet published — see
  [Publishing to the Marketplace](#publishing-to-the-marketplace) for the
  residual manual steps.

## Formatting

The server provides document (and range) formatting for `.lua`: it is
formatted in the canonical luabox style for the project's edition (SPEC §10).
The formatter never destroys code — a document with parse errors is simply
left unchanged. Range formatting has MVP semantics: the whole document is
formatted (the canonical formatter is whole-file by design).

For `.lua` files other Lua extensions may also register formatters, so pick
luabox explicitly if you want it:

```jsonc
"[lua]": {
  "editor.defaultFormatter": "luabox.luabox",
  "editor.formatOnSave": true
},
```

(`luabox.luabox` is this extension's ID: `publisher.name`.)

## Configuration

| Setting | Default | Description |
| --- | --- | --- |
| `luabox.path` | `luabox` | Path to the `luabox` executable. A bare name is resolved on `PATH`. The server is launched as `<path> lsp`. |
| `luabox.trace.server` | `off` | Trace LSP traffic (`off` / `messages` / `verbose`). |

## For maintainers: building and packaging

The rest of this section is for people building the extension itself, not
end users (who should follow [Getting the `luabox` binary](#getting-the-luabox-binary)
and [Installing the extension](#installing-the-extension) above).

### Build from source

```sh
cd editors/vscode
npm ci                    # or: npm install
npm run compile          # tsc -> ./out/extension.js
```

### Run in the Extension Development Host

Open `editors/vscode` in VS Code and press <kbd>F5</kbd> (Run Extension). A new
window opens with the extension loaded; open a `.lua` file to activate
the server.

### Package a `.vsix`

```sh
npm install -g @vscode/vsce      # or use: npx @vscode/vsce
npx @vscode/vsce package
```

This produces `luabox-0.1.0.vsix`, which can be installed via
`code --install-extension luabox-0.1.0.vsix` or **Extensions ▸ … ▸ Install from
VSIX** in the UI.

## Publishing to the Marketplace

Publishing requires credentials that are **not** bundled here:

1. Create a publisher named `luabox` at
   <https://marketplace.visualstudio.com/manage>.
2. Create an Azure DevOps Personal Access Token with the **Marketplace ▸
   Manage** scope.
3. `npx @vscode/vsce login luabox` (paste the PAT), then
   `npx @vscode/vsce publish`.

For Open VSX (used by VSCodium / Cursor / Gitpod), use `npx ovsx publish` with
an Open VSX token instead.

## Notes / known deviations

- The language server is launched as `luabox lsp` (no `--stdio` flag): the
  current CLI serves LSP over stdio unconditionally and rejects extra
  arguments. If a `--stdio` flag is added later, update `args` in
  `src/extension.ts`.
