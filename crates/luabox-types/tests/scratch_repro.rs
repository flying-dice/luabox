//! TEMPORARY reproduction tests for tickets #73/#74 — DELETE BEFORE DONE.

use std::path::PathBuf;

use luabox_diag::Diagnostic;
use luabox_syntax::lua::{Dialect, parse};
use luabox_types::{ShapeOptions, ShapeStore, Strictness, check_file_shaped, stdlib_defs};

fn strict_codes(source: &str) -> Vec<String> {
    let parsed = parse(source, Dialect::Lua54);
    assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
    check_file_shaped(
        &parsed,
        "test.lua",
        Strictness::Strict,
        None,
        Some(stdlib_defs(Dialect::Lua54)),
    )
    .iter()
    .map(|d| format!("{}: {}", d.code, d.message))
    .collect()
}

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        Fixture {
            dir: tempfile::tempdir().expect("tempdir"),
        }
    }
    fn write(&self, rel: &str, content: &str) {
        let path = self.dir.path().join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, content).expect("write");
    }
    fn check_strict(&self, source: &str) -> Vec<Diagnostic> {
        let parsed = parse(source, Dialect::Lua54);
        assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
        let store = ShapeStore::new(self.dir.path());
        let file_dir = self.dir.path().join("src");
        std::fs::create_dir_all(&file_dir).expect("mkdir");
        let opts = ShapeOptions {
            store: &store,
            file_dir: &file_dir,
            shape_paths: &Vec::<PathBuf>::new(),
            dependencies: &[],
        };
        check_file_shaped(
            &parsed,
            "src/main.lua",
            Strictness::Strict,
            Some(&opts),
            Some(stdlib_defs(Dialect::Lua54)),
        )
    }
    fn codes(&self, source: &str) -> Vec<String> {
        self.check_strict(source)
            .iter()
            .map(|d| format!("{}: {}", d.code, d.message))
            .collect()
    }
}

#[test]
fn repro_74_partial_annotation_misbinds() {
    // Annotate params 2..5 of 6: tags must bind by NAME, not position.
    let src = "\
---@param b string
---@param c boolean
---@param d integer
---@param e string
local function f(a, b, c, d, e, g)
  return a, b, c, d, e, g
end
f(1, \"s\", true, 2, \"t\", 3)
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn repro_73b_constructor_return_annotation() {
    let src = "\
---@class Circle
---@field radius number
local Circle = {}
Circle.__index = Circle

---@param radius number
---@return Circle
function Circle.new(radius)
  return setmetatable({ radius = radius }, Circle)
end
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn repro_73_struct_constructor() {
    let f = Fixture::new();
    f.write("src/geometry.luab", "struct Circle { radius: number }\n");
    let src = "\
---@use geometry

---@struct Circle
local Circle = {}
Circle.__index = Circle

---@param radius number
---@return Circle
function Circle.new(radius)
  return setmetatable({ radius = radius }, Circle)
end
";
    assert_eq!(f.codes(src), Vec::<String>::new());
}

#[test]
fn repro_73c_self_integer_field() {
    let f = Fixture::new();
    f.write("src/render.luab", "struct Square { side: integer }\n");
    let src = "\
---@use render

---@struct Square
local Square = {}
Square.__index = Square

function Square:draw()
  return string.rep(\"#\", self.side)
end

---@param side integer
---@return Square
function Square.new(side)
  return setmetatable({ side = side }, Square)
end
";
    assert_eq!(f.codes(src), Vec::<String>::new());
}
