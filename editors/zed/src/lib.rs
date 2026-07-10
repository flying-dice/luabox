//! Zed extension for luabox.
//!
//! Registers the `luabox lsp` stdio language server for Lua and `.luab` shape
//! files. The binary is resolved from the user's LSP settings (a `binary.path`
//! override) and otherwise from the worktree `PATH`.
//!
//! API verified against zed_extension_api 0.7 (docs.rs) and the Zed extension
//! docs (zed.dev/docs/extensions).

use zed_extension_api::settings::LspSettings;
use zed_extension_api::{self as zed, LanguageServerId, Result};

struct LuaboxExtension;

impl LuaboxExtension {
    /// Resolve the command that launches the luabox language server:
    /// 1. a `binary.path` (+ `binary.arguments`) override in settings.json, or
    /// 2. a `luabox` binary found on the worktree `PATH`, launched as
    ///    `luabox lsp`.
    fn server_command(
        &self,
        _language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        // 1. User override:
        //    "lsp": { "luabox": { "binary": { "path": "...", "arguments": [...] } } }
        if let Ok(lsp_settings) = LspSettings::for_worktree("luabox", worktree) {
            if let Some(binary) = lsp_settings.binary {
                if let Some(path) = binary.path {
                    return Ok(zed::Command {
                        command: path,
                        args: binary.arguments.unwrap_or_else(|| vec!["lsp".to_string()]),
                        env: worktree.shell_env(),
                    });
                }
            }
        }

        // 2. PATH fallback. `luabox lsp` speaks LSP over stdio unconditionally.
        let command = worktree.which("luabox").ok_or_else(|| {
            "`luabox` was not found on PATH. Install it (cargo build --release) \
             and add it to your PATH, or set \
             lsp.luabox.binary.path in your Zed settings."
                .to_string()
        })?;

        Ok(zed::Command {
            command,
            args: vec!["lsp".to_string()],
            env: worktree.shell_env(),
        })
    }
}

impl zed::Extension for LuaboxExtension {
    fn new() -> Self {
        LuaboxExtension
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        self.server_command(language_server_id, worktree)
    }
}

zed::register_extension!(LuaboxExtension);
