//! Unified type IR and inference engine — **Semantics** bounded context
//! (SPEC.md §3, §16).
//!
//! One internal type IR unifying LuaLS annotations and Luau native types.
//! Bidirectional inference, flow-sensitive narrowing, literal types, generics
//! with constraints. Strictness ladder: `none` → `warn` → `strict`.
//!
//! Status: placeholder — P1.
