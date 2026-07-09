//! Desugared IR and name resolution — the entry to the **Semantics** bounded
//! context (SPEC.md §16).
//!
//! Lowers `luabox-syntax` trees into a compact, dialect-neutral HIR and
//! resolves names (locals, upvalues, globals, `require` edges). Consumed by
//! `luabox-types` for inference and by `luabox-lower` for emit.
//!
//! Boundary contract: exposes HIR types and the syntax→HIR lowering API;
//! callers never see token-level details.
//!
//! Status: placeholder — P1.
