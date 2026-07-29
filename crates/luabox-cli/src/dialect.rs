//! The single unknown-dialect error path (`LB1001`), and the one place a
//! manifest dialect becomes a `luabox-syntax` dialect.
//!
//! Two facts about dialects meet here:
//!
//! 1. **A validated manifest cannot name an unknown dialect.** `[package]
//!    edition` and `[build] target` are parsed into
//!    [`luabox_manifest::model::DialectId`], a closed vocabulary, so every
//!    command that reads a manifest converts with the exhaustive match in
//!    [`from_manifest`] — no re-validation, and no "unknown edition in a
//!    manifest that already parsed" bail arm that could never fire.
//! 2. **A flag can.** `check --target`, `build --target` and `init`/`new
//!    --edition` take a spelling straight from the user. Those go through
//!    [`parse`], and a rejection is reported as `LB1001` with one message and
//!    one note, whichever command asked — as a rendered [`Diagnostic`] where
//!    the command has a diagnostic stream (`check`, so `--format json`
//!    carries it like any other finding), and as an ordinary error elsewhere.
//!
//! Distribution never parses syntax (SPEC.md §16), so `luabox-manifest` cannot
//! hand out a `luabox_syntax::Dialect` itself; this module is the frontend's
//! half of that boundary.

use luabox_diag::{Code, Diagnostic};
use luabox_manifest::model::DialectId;
use luabox_syntax::Dialect;

/// `LB1001` — unknown edition/target.
pub(crate) const UNKNOWN_DIALECT: Code = Code::new(1001);

/// The note every unknown-dialect report carries, so the reader is pointed at
/// the same explain page from every command.
const NOTE: &str = "run `luabox explain LB1001` for the full list of editions";

/// A dialect spelling the toolchain does not know, tagged with the thing that
/// supplied it (`"target"`, `"edition"`) so the message names the flag the
/// reader actually typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnknownDialect {
    what: &'static str,
    value: String,
}

impl std::fmt::Display for UnknownDialect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "unknown {} `{}`; expected one of: {}",
            self.what,
            self.value,
            DialectId::NAMES.join(", ")
        )
    }
}

impl UnknownDialect {
    /// The rendered `LB1001` diagnostic, for commands that have a diagnostic
    /// stream to put it on.
    pub(crate) fn diagnostic(&self) -> Diagnostic {
        Diagnostic::error(UNKNOWN_DIALECT, self.to_string()).with_note(NOTE)
    }
}

impl From<UnknownDialect> for anyhow::Error {
    /// The same message and note, for commands that fail with an error chain
    /// rather than a diagnostic report — so `?` on [`parse`] carries both.
    ///
    /// Deliberately *not* an `std::error::Error` impl: that would hand the job
    /// to `anyhow`'s blanket conversion, which renders `Display` alone and so
    /// would drop the note on exactly the commands (`build`, `init`, `new`)
    /// that have no diagnostic stream to put it on.
    fn from(unknown: UnknownDialect) -> Self {
        anyhow::anyhow!("{unknown}\nnote: {NOTE}")
    }
}

/// Parse a user-supplied dialect spelling (`--target`, `--edition`).
pub(crate) fn parse(what: &'static str, id: &str) -> Result<Dialect, UnknownDialect> {
    id.parse::<DialectId>()
        .map(from_manifest)
        .map_err(|unknown| UnknownDialect {
            what,
            value: unknown.value,
        })
}

/// The `luabox-syntax` dialect a validated manifest field names.
///
/// Exhaustive by construction: adding a dialect to either vocabulary stops
/// this compiling, which is the point — the two lists cannot drift.
pub(crate) fn from_manifest(id: DialectId) -> Dialect {
    match id {
        DialectId::Lua51 => Dialect::Lua51,
        DialectId::Lua52 => Dialect::Lua52,
        DialectId::Lua53 => Dialect::Lua53,
        DialectId::Lua54 => Dialect::Lua54,
        DialectId::LuaJit => Dialect::LuaJit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sync test the two vocabularies never had: `luabox-manifest`'s
    /// allow-list and `luabox-syntax`'s dialect set are declared in different
    /// crates that cannot see each other (SPEC.md §16), so `luabox-cli` — the
    /// only place that sees both — is where they are pinned to each other.
    #[test]
    fn the_manifest_dialect_vocabulary_and_the_syntax_dialects_are_the_same_set() {
        assert_eq!(
            DialectId::ALL.len(),
            Dialect::ALL.len(),
            "one vocabulary grew a dialect the other does not have"
        );
        for id in DialectId::ALL {
            let dialect = from_manifest(*id);
            assert_eq!(
                Dialect::from_manifest_id(id.as_str()),
                Some(dialect),
                "`{id}` must name the same dialect on both sides"
            );
            assert_eq!(
                dialect.manifest_id(),
                id.as_str(),
                "`{id}` must render back to the manifest spelling"
            );
        }
        // ...and nothing in `Dialect` is missing a manifest spelling.
        for dialect in Dialect::ALL {
            assert!(
                DialectId::NAMES.contains(&dialect.manifest_id()),
                "`{}` has no `luabox.toml` spelling",
                dialect.manifest_id()
            );
        }
    }

    #[test]
    fn a_known_spelling_parses_to_its_dialect() {
        assert_eq!(parse("target", "luajit"), Ok(Dialect::LuaJit));
    }

    #[test]
    fn an_unknown_spelling_names_the_flag_and_lists_every_dialect() {
        let error = parse("target", "6.0").expect_err("6.0 is not a dialect");
        assert_eq!(
            error.to_string(),
            "unknown target `6.0`; expected one of: 5.1, 5.2, 5.3, 5.4, luajit"
        );

        let error = parse("edition", "luau").expect_err("luau is not a dialect");
        assert!(error.to_string().starts_with("unknown edition `luau`"));
    }

    #[test]
    fn the_diagnostic_and_the_error_carry_the_same_message_and_note() {
        let error = parse("target", "6.0").expect_err("6.0 is not a dialect");
        let diagnostic = error.diagnostic();
        assert_eq!(diagnostic.code, UNKNOWN_DIALECT);
        assert_eq!(diagnostic.message, error.to_string());
        assert_eq!(diagnostic.notes, vec![NOTE.to_owned()]);

        let rendered = anyhow::Error::from(error).to_string();
        assert!(rendered.starts_with("unknown target `6.0`"), "{rendered}");
        assert!(rendered.contains(NOTE), "{rendered}");
    }
}
