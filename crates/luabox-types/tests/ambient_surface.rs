// test code — panics document assumptions
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The ambient layer's read surface, pinned from the crate's own suite
//! (#58): these behaviours were previously exercised only through the
//! cli crate's e2e suites, so the #58 mutation pass found the defs-layer
//! half of [`TypeEnv::clone_surface`] and the whole of
//! [`Ambient::class_members`] unprotected — a mutant could drop every
//! defs class/enum/function/global from the merged layer, or empty the
//! editor's member surface, and `cargo test -p luabox-types` stayed green.
//!
//! Two axes, deliberately separated:
//!
//! - **the defs layer survives the merge** — [`Ambient::with_project_types`]
//!   clones the base surface (`clone_surface`) before merging project
//!   files, and each of the four cloned maps (classes, enums, functions,
//!   global types) is probed through a diagnostic only it can produce;
//! - **`class_members` is the checker's shape** — the editor surfaces
//!   (#56) resolve members through it, so it must carry `---@field`s,
//!   carrier attachments, and the parent fold, and decline non-classes.

use luabox_syntax::lua::{Dialect, parse};
use luabox_types::{Ambient, Strictness, build_ambient, check_file_with_ambient, module_surface};

/// A definition package touching all four cloned maps: a class (with a
/// parent), an enum, a module function, and a typed scalar global.
const DEFS: &str = "\
---@meta

---@class DefBase
---@field id number

---@class DefWidget : DefBase
---@field label string

---@enum DefMode
local DefMode = {
  fast = 'fast',
  slow = 'slow',
}

---@param n number
---@return string
function deflib.render(n) end

---@type number
DEF_LIMIT = 0
";

fn defs_ambient() -> Ambient {
    build_ambient(Dialect::Lua54, &[DEFS.to_string()])
}

/// The defs ambient with the project-types merge applied over NO files —
/// the merge whose clone must preserve the base surface.
fn merged() -> Ambient {
    defs_ambient().with_project_types([])
}

fn codes(src: &str, ambient: &Ambient) -> Vec<String> {
    let parsed = parse(src, Dialect::Lua54);
    assert_eq!(parsed.errors(), &[], "fixture must parse cleanly:\n{src}");
    check_file_with_ambient(
        &parsed,
        "test.lua",
        Strictness::Strict,
        Dialect::Lua54,
        Some(ambient),
    )
    .iter()
    .map(|d| d.code.to_string())
    .collect()
}

// --- the defs layer survives `with_project_types`' clone -------------------

#[test]
fn defs_classes_survive_the_project_merge() {
    let ambient = merged();
    let src = "\
---@type DefWidget
local w
print(w.nope)
";
    assert_eq!(
        codes(src, &ambient),
        vec!["LB0306"],
        "an undeclared member on a defs class must still be diagnosable \
         after the merge — losing it means the cloned surface dropped the \
         class map"
    );
}

#[test]
fn defs_functions_survive_the_project_merge() {
    let ambient = merged();
    let src = "local s = deflib.render('not a number')\n";
    assert_eq!(
        codes(src, &ambient),
        vec!["LB0300"],
        "a defs function's signature must still argument-check after the merge"
    );
}

#[test]
fn defs_enums_survive_the_project_merge() {
    let ambient = merged();
    let src = "\
---@param m DefMode
local function use(m) end
use('nonsense')
";
    assert_eq!(
        codes(src, &ambient),
        vec!["LB0300"],
        "a defs enum must still constrain values after the merge"
    );
}

#[test]
fn defs_global_types_survive_the_project_merge() {
    let ambient = merged();
    // Both directions, deliberately: strict mode rejects `unknown` at a
    // typed parameter with the same LB0300 a real mismatch gets, so the
    // rejecting probe alone cannot tell `number` from a dropped global-type
    // map. The ACCEPTING probe is the separating one: it is clean only if
    // the global still carries its declared type.
    let clean = "\
---@param n number
local function want(n) end
want(DEF_LIMIT)
";
    assert_eq!(
        codes(clean, &ambient),
        Vec::<String>::new(),
        "a defs-typed global must keep its declared type after the merge"
    );
    let caught = "\
---@param s string
local function want(s) end
want(DEF_LIMIT)
";
    assert_eq!(
        codes(caught, &ambient),
        vec!["LB0300"],
        "…and the type must be the declared one, not merely something"
    );
}

// --- `class_members`: the editor surfaces' read path (#56) -----------------

#[test]
fn class_members_carries_declared_fields_and_the_parent_fold() {
    let ambient = merged();
    let shape = ambient
        .class_members("DefWidget")
        .expect("DefWidget is a class this layer declares");
    assert!(
        shape.fields.contains_key("label"),
        "own ---@field missing: {:?}",
        shape.fields.keys().collect::<Vec<_>>()
    );
    assert!(
        shape.fields.contains_key("id"),
        "parent (DefBase) field not folded in: {:?}",
        shape.fields.keys().collect::<Vec<_>>()
    );
}

#[test]
fn class_members_carries_carrier_attachments_from_project_files() {
    // The #56 shape end-to-end: a project file's carrier attachments must be
    // readable through the merged layer, because that is what hover and
    // completion offer for a require'd class module.
    let module = "\
---@class Codec
local Codec = {}

---@param s string
---@return string
function Codec.encode(s)
  return s
end

return Codec
";
    let parsed = parse(module, Dialect::Lua54);
    assert_eq!(parsed.errors(), &[], "module fixture must parse cleanly");
    let base = defs_ambient();
    let types = module_surface(&parsed, "codec.lua", Some(&base)).types;
    let ambient = base.with_project_types([&types]);
    let shape = ambient
        .class_members("Codec")
        .expect("the project file's class is workspace-global");
    assert!(
        shape.fields.contains_key("encode"),
        "carrier attachment missing: {:?}",
        shape.fields.keys().collect::<Vec<_>>()
    );
}

#[test]
fn class_members_declines_a_name_that_is_not_a_class() {
    let ambient = merged();
    assert!(ambient.class_members("NoSuchClass").is_none());
    assert!(
        ambient.class_members("DefMode").is_none(),
        "an enum is not a class — `Some(empty)` here would make the editor \
         offer nothing while claiming the name resolves"
    );
}
