# luabox for Zed

A [Zed](https://zed.dev) extension that adds the **luabox language server**
(`luabox lsp`) for Lua files — typecheck, hover, goto-definition, completion,
document symbols, formatting and semantic highlighting.

## Requirements

- A `luabox` binary on your `PATH`, or a path set in Zed settings (below).
  The server is launched as `luabox lsp`. Get the binary via the install
  script rather than building from source — see [Getting the `luabox`
  binary](#getting-the-luabox-binary) below.

## Getting the `luabox` binary

From a released build, run the install script for your platform (see the
repo root [`RELEASING.md`](../../RELEASING.md) for how releases are cut):

```sh
# Linux / macOS
curl -fsSL https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/raw/main/scripts/install.sh | bash
```

```powershell
# Windows
irm https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/raw/main/scripts/install.ps1 | iex
```

Both scripts fetch the latest tagged GitLab Release's binary asset and place
it on `PATH` (override the install directory with `LUABOX_INSTALL_DIR` /
`-InstallDir`). Until the first `v*` tag is pushed, both print a
`cargo install --git` source-build fallback instead.

## How it attaches to Lua

Zed ships Lua support already. This extension declares the luabox server for
the `Lua` language in `extension.toml`, so Zed makes it **available**
automatically alongside any other Lua server — no settings edit and no
`languages/lua/` override that would clobber Zed's built-in Lua highlighting.

To control ordering or disable another server, add to your `settings.json`:

```json
{
  "languages": {
    "Lua": { "language_servers": ["luabox", "..."] }
  }
}
```

(`"..."` expands to the remaining available servers; prefix a name with `"!"`
to disable it.)

## Configuring the binary path

Point Zed at a specific `luabox` binary (and/or override the args) in
`settings.json`:

```json
{
  "lsp": {
    "luabox": {
      "binary": {
        "path": "/absolute/path/to/luabox",
        "arguments": ["lsp"]
      }
    }
  }
}
```

Without an override the extension resolves `luabox` on the worktree `PATH` and
runs `luabox lsp`.

## Install the extension

Zed does not yet have a published registry entry for this extension (see
[Publishing to the Zed extension registry](#publishing-to-the-zed-extension-registry)
for the residual steps), so for now install it as a **dev extension** from a
local checkout:

1. Build the wasm (sanity check — Zed also compiles it on install):

   ```sh
   rustup target add wasm32-wasip2
   cd editors/zed
   cargo build --target wasm32-wasip2 --release
   ```

2. In Zed: **Extensions ▸ Install Dev Extension**, and select this
   `editors/zed` directory.

## Publishing to the Zed extension registry

Extensions are published by PR to
[`zed-industries/extensions`](https://github.com/zed-industries/extensions):

1. Host this extension in a **public git repo** with a license file at its root
   (MIT/Apache-2.0/BSD — mandatory).
2. Fork `zed-industries/extensions`; `git submodule init && git submodule update`.
3. Add your repo as a submodule:
   `git submodule add <url> extensions/luabox`.
4. Add to the top-level `extensions.toml`:

   ```toml
   [luabox]
   submodule = "extensions/luabox"
   version = "0.1.0"   # must match extension.toml at that commit
   ```

5. Run `pnpm sort-extensions`, open a PR. On merge Zed packages and publishes
   it. Update later with `git submodule update --remote` + a version bump PR.

Because this repo is self-hosted on a tailnet, publishing to the public
registry additionally requires mirroring this extension to a public git host
and updating the `repository` URL in `extension.toml`.

## References

- [Zed — Developing Extensions](https://zed.dev/docs/extensions/developing-extensions)
- [Zed — Language Extensions](https://zed.dev/docs/extensions/languages)
- [`zed_extension_api`](https://docs.rs/zed_extension_api)
