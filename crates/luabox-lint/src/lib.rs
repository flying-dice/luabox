//! Type-informed lint rules — the clippy analog (SPEC.md §9).
//!
//! Consumer crate of the Semantics context (like `luabox-lsp`), added
//! alongside the SPEC §16 crate list the way clippy sits beside rustc.
//! All rules run over the same parse/HIR/type machinery as `check` —
//! no regex lints. Tiers: correctness (deny), suspicious, perf, style,
//! pedantic (opt-in). `--fix` applies machine-applicable fixes via the
//! lossless tree.
//!
//! Status: under construction — ticket #15.
