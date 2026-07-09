//! Target lowering and polyfill injection — the tsc bit. **Emit** bounded
//! context (SPEC.md §2.1, §16).
//!
//! Lowers the `edition` (dialect you write) to the `target` (dialect you
//! ship): goto→5.1 restructuring, bitop/`//` shims, `<close>`/`<const>`
//! rewrites, `_ENV`→setfenv, LuaJIT `bit.*` shims; tree-shaken `__luabox_rt`
//! polyfill module. Takes checked HIR in, bytes out; cannot influence
//! checking. Shape-blind by invariant: output is byte-identical with or
//! without `.lb` files (SHAPES.md §1).
//!
//! Status: placeholder — P3.
