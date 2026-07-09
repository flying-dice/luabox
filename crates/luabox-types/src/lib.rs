//! Unified type IR and inference engine — **Semantics** bounded context
//! (SPEC.md §3, §16).
//!
//! One internal type IR fed by two front-ends: LuaCATS annotations
//! (`---@class` etc., full compatibility non-negotiable) and the `.lb`
//! shape DSL (SHAPES.md — sealed structs, traits, coherence). One checker,
//! no parallel type system; interop between the front-ends is total.
//!
//! Bidirectional inference, flow-sensitive narrowing, literal types,
//! generics with constraints. Rich table inference is a hard requirement:
//! tables never degrade to a bare `table` type (SPEC.md §3).
//! Strictness ladder: `none` → `warn` → `strict`; shape rules are hard
//! errors at every level.
//!
//! Status: placeholder — P1.
