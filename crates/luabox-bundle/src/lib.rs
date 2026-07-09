//! Bundler: require-graph resolution, tree-shaking, minify, sourcemaps —
//! **Emit** bounded context (SPEC.md §7, §16).
//!
//! Single `.lua` file per entry, target-lowered; static `require` inlined
//! into a lazy-init module map preserving load-order and cycles; scope-aware
//! identifier mangling (property names never mangled); `.lua.map` sourcemaps.
//!
//! Status: placeholder — P3.
