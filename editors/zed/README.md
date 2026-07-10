# luabox for Zed

A [Zed](https://zed.dev) extension that adds:

- the **luabox language server** (`luabox lsp`) for both Lua and `.luab` shape
  files — typecheck, hover, goto-definition, completion, document symbols,
  formatting and semantic highlighting; and
- a **`LuaBox Shape` language** for `.luab` files, with a tree-sitter grammar
  and syntax highlighting.

## Requirements

- A `luabox` binary on your `PATH` (build it from the repo root with
  `cargo build --release`; binary: `target/release/luabox`), or a path set in
  Zed settings (below). The server is launched as `luabox lsp`.

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

## Install as a dev extension

1. Build the wasm (sanity check — Zed also compiles it on install):

   ```sh
   rustup target add wasm32-wasip2
   cd editors/zed
   cargo build --target wasm32-wasip2 --release
   ```

2. In Zed: **Extensions ▸ Install Dev Extension**, and select this
   `editors/zed` directory.

> **Grammar note.** Zed fetches tree-sitter grammars from a **git repository at
> a fixed rev** — a repo + rev is mandatory; there is no "build from a plain
> local folder" mode. The `.luab` grammar lives in this monorepo under
> [`tree-sitter-luab/`](../../tree-sitter-luab), referenced from
> `extension.toml` via `repository` + `path = "tree-sitter-luab"` + `rev`.
> **The `rev` must be a real pushed commit SHA** — update it after every
> grammar change. For local dev, point `repository` at a `file://` URL of your
> local checkout (still at a committed rev).

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
registry additionally requires mirroring both this extension **and** the
`tree-sitter-luab` grammar to public git hosts and updating the `repository`
URLs in `extension.toml`.

## References

- [Zed — Developing Extensions](https://zed.dev/docs/extensions/developing-extensions)
- [Zed — Language Extensions](https://zed.dev/docs/extensions/languages)
- [`zed_extension_api`](https://docs.rs/zed_extension_api)
