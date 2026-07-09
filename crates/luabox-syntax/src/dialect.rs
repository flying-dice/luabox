/// A Lua dialect luabox can read (`edition`) or emit (`target`).
///
/// See SPEC.md §2 for the full support matrix. The lexer lexes the *union*
/// of all dialects wherever that is unambiguous (dialect-illegal constructs
/// are diagnosed later, with spans, rather than mangled at token level); the
/// only token-level divergences are the keyword set and Luau-specific string
/// syntax, which this type gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Dialect {
    Lua51,
    Lua52,
    Lua53,
    Lua54,
    /// LuaJIT: 5.1 plus extensions (`goto`, hex floats, `LL`/`ULL`/`i`
    /// number suffixes).
    LuaJit,
    Luau,
}

impl Dialect {
    /// `goto`/`::label::` are part of the grammar (5.2+, LuaJIT).
    /// Where false, `goto` lexes as a plain identifier.
    pub fn has_goto(self) -> bool {
        !matches!(self, Dialect::Lua51 | Dialect::Luau)
    }

    /// Luau-only lexical syntax: backtick interpolated strings, compound
    /// assignment operators, `->` in type position.
    pub fn is_luau(self) -> bool {
        self == Dialect::Luau
    }

    /// The identifier used in `luabox.toml` (`edition = "5.4"`).
    pub fn manifest_id(self) -> &'static str {
        match self {
            Dialect::Lua51 => "5.1",
            Dialect::Lua52 => "5.2",
            Dialect::Lua53 => "5.3",
            Dialect::Lua54 => "5.4",
            Dialect::LuaJit => "luajit",
            Dialect::Luau => "luau",
        }
    }

    /// Parse a `luabox.toml` dialect id.
    pub fn from_manifest_id(id: &str) -> Option<Self> {
        Some(match id {
            "5.1" => Dialect::Lua51,
            "5.2" => Dialect::Lua52,
            "5.3" => Dialect::Lua53,
            "5.4" => Dialect::Lua54,
            "luajit" => Dialect::LuaJit,
            "luau" => Dialect::Luau,
            _ => return None,
        })
    }
}
