//! `luabox lsp` — start the language server on stdio (SPEC.md §8).
//!
//! Thin frontend: the mainloop, capabilities, and analysis wiring live in
//! `luabox-lsp`. The transport is stdio (the editor default); the server
//! runs until the client completes the `shutdown`/`exit` handshake.

pub fn run() -> anyhow::Result<()> {
    luabox_lsp::run_stdio()
}
