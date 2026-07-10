//! Shape checking end-to-end (SHAPES.md §4–§6): every `LB2xxx` rule firing
//! and not firing, resolution tiers and ambiguity, generics bounds,
//! supertraits, LuaCATS interop, the `Result<T, E>` convention, sealed vs
//! open (`..`), and `setmetatable` instantiation.
//!
//! Fixtures are real temp-dir projects: `.lb` files on disk, `.lua` sources
//! checked through the public [`luabox_types::check_file_shaped`] API.

use std::path::PathBuf;

use luabox_diag::Diagnostic;
use luabox_syntax::lua::{Dialect, parse};
use luabox_types::{ShapeOptions, ShapeStore, Strictness, check_file_shaped};

struct Fixture {
    dir: tempfile::TempDir,
    shape_paths: Vec<PathBuf>,
}

impl Fixture {
    fn new() -> Self {
        Fixture {
            dir: tempfile::tempdir().expect("tempdir"),
            shape_paths: Vec::new(),
        }
    }

    fn write(&self, rel: &str, content: &str) {
        let path = self.dir.path().join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, content).expect("write");
    }

    fn add_shape_path(&mut self, rel: &str) {
        let path = self.dir.path().join(rel);
        std::fs::create_dir_all(&path).expect("mkdir");
        self.shape_paths.push(path);
    }

    /// Check `source` as `src/main.lua` at the given strictness.
    fn check_at(&self, source: &str, strictness: Strictness) -> Vec<Diagnostic> {
        let parsed = parse(source, Dialect::Lua54);
        assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
        let store = ShapeStore::new(self.dir.path());
        let file_dir = self.dir.path().join("src");
        std::fs::create_dir_all(&file_dir).expect("mkdir");
        let opts = ShapeOptions {
            store: &store,
            file_dir: &file_dir,
            shape_paths: &self.shape_paths,
        };
        check_file_shaped(&parsed, "src/main.lua", strictness, Some(&opts))
    }

    fn check(&self, source: &str) -> Vec<Diagnostic> {
        self.check_at(source, Strictness::Warn)
    }

    fn codes(&self, source: &str) -> Vec<String> {
        self.check(source)
            .iter()
            .map(|d| d.code.to_string())
            .collect()
    }

    /// Check a `.lb` file previously written with [`Fixture::write`].
    fn check_lb(&self, rel: &str) -> Vec<Diagnostic> {
        let path = self.dir.path().join(rel);
        let source = std::fs::read_to_string(&path).expect("read .lb");
        let store = ShapeStore::new(self.dir.path());
        store.check_lb_file(&path, &source, &self.shape_paths)
    }
}

fn geometry_fixture() -> Fixture {
    let f = Fixture::new();
    f.write(
        "src/geometry.lb",
        "struct Point { x: number, y: number, label: string? }\n\
         struct Bag { n: number, .. }\n",
    );
    f
}

// === Sealed checking (LB2001 / LB2002) ====================================

#[test]
fn lb2001_missing_field_on_bound_literal() {
    let f = geometry_fixture();
    let diags = f.check("---@use geometry\n\n---@struct Point\nlocal p = { x = 0 }\n");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code.to_string(), "LB2001");
    assert!(diags[0].message.contains("`y`"), "{}", diags[0].message);
}

#[test]
fn lb2001_optional_field_may_be_omitted() {
    let f = geometry_fixture();
    let diags = f.check("---@use geometry\n\n---@struct Point\nlocal p = { x = 0, y = 1 }\n");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn lb2002_unknown_key_in_bound_literal() {
    let f = geometry_fixture();
    let diags =
        f.check("---@use geometry\n\n---@struct Point\nlocal p = { x = 0, y = 0, z = 0 }\n");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code.to_string(), "LB2002");
    assert!(diags[0].message.contains("`z`"), "{}", diags[0].message);
}

#[test]
fn lb2002_unknown_key_write_and_read() {
    let f = geometry_fixture();
    let src = "\
---@use geometry

---@struct Point
local p = { x = 0, y = 0 }
p.z = 1
print(p.w)
";
    let codes = f.codes(src);
    assert_eq!(codes, vec!["LB2002", "LB2002"]);
}

#[test]
fn known_field_reads_and_writes_are_clean() {
    let f = geometry_fixture();
    let src = "\
---@use geometry

---@struct Point
local p = { x = 0, y = 0 }
p.x = 2
p.label = \"origin\"
print(p.y, p.label)
";
    assert!(f.check(src).is_empty());
}

#[test]
fn field_write_type_mismatch_is_lb0300_at_strictness() {
    let f = geometry_fixture();
    let src = "\
---@use geometry

---@struct Point
local p = { x = 0, y = 0 }
p.x = \"nope\"
";
    let warn = f.check(src);
    assert_eq!(warn.len(), 1, "{warn:?}");
    assert_eq!(warn[0].code.to_string(), "LB0300");
    assert_eq!(warn[0].severity, luabox_diag::Severity::Warning);
    let strict = f.check_at(src, Strictness::Strict);
    assert_eq!(strict[0].severity, luabox_diag::Severity::Error);
}

#[test]
fn open_struct_accepts_extra_keys() {
    let f = geometry_fixture();
    let src = "\
---@use geometry

---@struct Bag
local b = { n = 1, extra = true }
b.more = 2
print(b.other)
";
    assert!(f.check(src).is_empty(), "{:?}", f.check(src));
}

#[test]
fn sealed_field_value_types_checked_in_literal() {
    let f = geometry_fixture();
    let src = "\
---@use geometry

---@struct Point
local p = { x = \"no\", y = 0 }
";
    let diags = f.check_at(src, Strictness::Strict);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code.to_string(), "LB0300");
}

#[test]
fn lb2xxx_fire_even_at_strictness_none() {
    let f = geometry_fixture();
    let diags = f.check_at(
        "---@use geometry\n\n---@struct Point\nlocal p = { x = 0 }\n",
        Strictness::None,
    );
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code.to_string(), "LB2001");
    assert_eq!(diags[0].severity, luabox_diag::Severity::Error);
}

// === LB2006: undeclared struct =============================================

#[test]
fn lb2006_undeclared_struct() {
    let f = geometry_fixture();
    let codes = f.codes("---@use geometry\n\n---@struct Wibble\nlocal w = {}\n");
    assert_eq!(codes, vec!["LB2006"]);
}

#[test]
fn declared_struct_is_not_lb2006() {
    let f = geometry_fixture();
    assert!(
        f.check("---@use geometry\n\n---@struct Point\nlocal p = { x = 0, y = 0 }\n")
            .is_empty()
    );
}

// === Instantiation: setmetatable(literal, Carrier) ========================

fn circle_fixture() -> Fixture {
    let f = Fixture::new();
    f.write(
        "src/geometry.lb",
        "struct Circle { radius: number }\n\
         trait Shape {\n    fn area(self) -> number;\n    fn perimeter(self) -> number;\n}\n",
    );
    f
}

const CIRCLE_CARRIER: &str = "\
---@use geometry

---@struct Circle
local Circle = {}
Circle.__index = Circle

---@impl Shape for Circle
function Circle:area()
  return 3.14 * self.radius * self.radius
end

function Circle:perimeter()
  return 2 * 3.14 * self.radius
end
";

#[test]
fn carrier_with_methods_is_clean() {
    let f = circle_fixture();
    let src = format!(
        "{CIRCLE_CARRIER}\nfunction Circle.new(radius)\n  return setmetatable({{ radius = radius }}, Circle)\nend\n"
    );
    let diags = f.check(&src);
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn setmetatable_literal_missing_field_is_lb2001() {
    let f = circle_fixture();
    let src = format!(
        "{CIRCLE_CARRIER}\nfunction Circle.new()\n  return setmetatable({{}}, Circle)\nend\n"
    );
    let diags = f.check(&src);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code.to_string(), "LB2001");
    assert!(diags[0].message.contains("`radius`"));
}

#[test]
fn setmetatable_literal_unknown_key_is_lb2002() {
    let f = circle_fixture();
    let src =
        format!("{CIRCLE_CARRIER}\nlocal c = setmetatable({{ radius = 1, wobble = 2 }}, Circle)\n");
    let codes: Vec<String> = f.check(&src).iter().map(|d| d.code.to_string()).collect();
    assert_eq!(codes, vec!["LB2002"]);
}

#[test]
fn setmetatable_result_is_a_sealed_instance() {
    let f = circle_fixture();
    let src = format!(
        "{CIRCLE_CARRIER}\nlocal c = setmetatable({{ radius = 1 }}, Circle)\nprint(c.radius)\nprint(c:area())\nprint(c.oops)\n"
    );
    let diags = f.check(&src);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code.to_string(), "LB2002");
    assert!(diags[0].message.contains("`oops`"));
}

#[test]
fn self_in_methods_is_sealed_too() {
    let f = circle_fixture();
    let src = "\
---@use geometry

---@struct Circle
local Circle = {}
Circle.__index = Circle

---@impl Shape for Circle
function Circle:area()
  return self.radius * self.typo
end

function Circle:perimeter()
  return 0
end
";
    let diags = f.check(src);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code.to_string(), "LB2002");
    assert!(diags[0].message.contains("`typo`"));
}

// === Trait coherence (LB2003 / LB2004 / LB2008) ============================

#[test]
fn lb2003_incomplete_impl_lists_missing_fns() {
    let f = circle_fixture();
    let src = "\
---@use geometry

---@struct Circle
local Circle = {}
Circle.__index = Circle

---@impl Shape for Circle
function Circle:area()
  return 1
end
";
    let diags = f.check(src);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code.to_string(), "LB2003");
    assert!(
        diags[0].message.contains("`perimeter`"),
        "{}",
        diags[0].message
    );
}

#[test]
fn extra_inherent_methods_are_fine() {
    let f = circle_fixture();
    let src = format!("{CIRCLE_CARRIER}\nfunction Circle:describe()\n  return \"a circle\"\nend\n");
    assert!(f.check(&src).is_empty());
}

#[test]
fn lb2004_return_covariance_violation_has_both_spans() {
    let f = circle_fixture();
    let src = "\
---@use geometry

---@struct Circle
local Circle = {}
Circle.__index = Circle

---@impl Shape for Circle
---@return string
function Circle:area()
  return \"round\"
end

function Circle:perimeter()
  return 0
end
";
    let diags = f.check(src);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code.to_string(), "LB2004");
    let primary = diags[0].primary_label().expect("primary");
    assert_eq!(primary.span.file, "src/main.lua");
    let secondary = diags[0]
        .labels
        .iter()
        .find(|l| !l.primary)
        .expect("secondary label");
    assert_eq!(secondary.span.file, "src/geometry.lb");
}

#[test]
fn lb2004_receiver_mismatch() {
    let f = circle_fixture();
    let src = "\
---@use geometry

---@struct Circle
local Circle = {}
Circle.__index = Circle

---@impl Shape for Circle
function Circle.area()
  return 1
end

function Circle:perimeter()
  return 0
end
";
    let codes: Vec<String> = f.check(src).iter().map(|d| d.code.to_string()).collect();
    assert_eq!(codes, vec!["LB2004"]);
}

#[test]
fn dot_decl_with_explicit_self_satisfies_receiver() {
    let f = circle_fixture();
    let src = "\
---@use geometry

---@struct Circle
local Circle = {}
Circle.__index = Circle

---@impl Shape for Circle
function Circle.area(self)
  return 1
end

function Circle:perimeter()
  return 0
end
";
    assert!(f.check(src).is_empty(), "{:?}", f.check(src));
}

#[test]
fn lb2004_param_arity_mismatch() {
    let f = circle_fixture();
    let src = "\
---@use geometry

---@struct Circle
local Circle = {}
Circle.__index = Circle

---@impl Shape for Circle
function Circle:area(extra)
  return 1
end

function Circle:perimeter()
  return 0
end
";
    let codes: Vec<String> = f.check(src).iter().map(|d| d.code.to_string()).collect();
    assert_eq!(codes, vec!["LB2004"]);
}

#[test]
fn lb2004_param_contravariance_violation() {
    let f = Fixture::new();
    f.write(
        "src/geometry.lb",
        "struct Circle { radius: number }\n\
         trait Scalable {\n    fn scale(self, factor: number);\n}\n",
    );
    let src = "\
---@use geometry

---@struct Circle
local Circle = {}
Circle.__index = Circle

---@impl Scalable for Circle
---@param factor string
function Circle:scale(factor)
end
";
    let codes: Vec<String> = f.check(src).iter().map(|d| d.code.to_string()).collect();
    assert_eq!(codes, vec!["LB2004"]);
}

#[test]
fn annotated_matching_signature_is_clean() {
    let f = Fixture::new();
    f.write(
        "src/geometry.lb",
        "struct Circle { radius: number }\n\
         trait Scalable {\n    fn scale(self, factor: number) -> number;\n}\n",
    );
    let src = "\
---@use geometry

---@struct Circle
local Circle = {}
Circle.__index = Circle

---@impl Scalable for Circle
---@param factor number
---@return number
function Circle:scale(factor)
  return factor
end
";
    assert!(f.check(src).is_empty(), "{:?}", f.check(src));
}

#[test]
fn lb2008_supertrait_conformance_missing() {
    let f = Fixture::new();
    f.write(
        "src/geometry.lb",
        "struct Circle { radius: number }\n\
         trait Shape {\n    fn area(self) -> number;\n}\n\
         trait Drawable: Shape {\n    fn draw(self);\n}\n",
    );
    let src = "\
---@use geometry

---@struct Circle
local Circle = {}
Circle.__index = Circle

---@impl Drawable for Circle
function Circle:draw()
end
";
    let codes: Vec<String> = f.check(src).iter().map(|d| d.code.to_string()).collect();
    assert_eq!(codes, vec!["LB2008"]);
}

#[test]
fn lb2008_satisfied_by_impl_on_same_carrier() {
    let f = Fixture::new();
    f.write(
        "src/geometry.lb",
        "struct Circle { radius: number }\n\
         trait Shape {\n    fn area(self) -> number;\n}\n\
         trait Drawable: Shape {\n    fn draw(self);\n}\n",
    );
    let src = "\
---@use geometry

---@struct Circle
local Circle = {}
Circle.__index = Circle

---@impl Shape for Circle
function Circle:area()
  return 1
end

---@impl Drawable for Circle
function Circle:draw()
end
";
    assert!(f.check(src).is_empty(), "{:?}", f.check(src));
}

#[test]
fn lb2008_satisfied_by_lb_impl_assertion() {
    let f = Fixture::new();
    f.write(
        "src/geometry.lb",
        "struct Circle { radius: number }\n\
         trait Shape {\n    fn area(self) -> number;\n}\n\
         trait Drawable: Shape {\n    fn draw(self);\n}\n\
         impl Shape for Circle;\n",
    );
    let src = "\
---@use geometry

---@struct Circle
local Circle = {}
Circle.__index = Circle

---@impl Drawable for Circle
function Circle:draw()
end
";
    assert!(f.check(src).is_empty(), "{:?}", f.check(src));
}

#[test]
fn lb2006_impl_of_undeclared_trait_or_struct() {
    let f = circle_fixture();
    let src = "\
---@use geometry

local Thing = {}

---@impl Nope for Circle
function Thing:x() end
";
    let codes: Vec<String> = f.check(src).iter().map(|d| d.code.to_string()).collect();
    assert_eq!(codes, vec!["LB2006"]);

    let src = "\
---@use geometry

local Thing = {}

---@impl Shape for Missing
function Thing:x() end
";
    let codes: Vec<String> = f.check(src).iter().map(|d| d.code.to_string()).collect();
    assert_eq!(codes, vec!["LB2006"]);
}

// === Interop with LuaCATS ===================================================

#[test]
fn luacats_class_satisfies_a_shape_trait() {
    let f = Fixture::new();
    f.write(
        "src/geometry.lb",
        "trait Shape {\n    fn area(self) -> number;\n}\n",
    );
    let src = "\
---@use geometry

---@class Square
---@field side number
local Square = {}
Square.__index = Square

---@impl Shape for Square
function Square:area()
  return self.side * self.side
end
";
    assert!(f.check(src).is_empty(), "{:?}", f.check(src));
}

#[test]
fn luacats_class_field_fn_type_counts_as_method() {
    let f = Fixture::new();
    f.write(
        "src/geometry.lb",
        "trait Shape {\n    fn area(self) -> number;\n}\n",
    );
    let src = "\
---@use geometry

---@class Square
---@field side number
---@field area fun(self: Square): number
local Square = {}

---@impl Shape for Square
local _ = Square
";
    assert!(f.check(src).is_empty(), "{:?}", f.check(src));
}

#[test]
fn shape_struct_usable_in_luacats_annotations() {
    let f = geometry_fixture();
    let src = "\
---@use geometry

---@param p Point
---@return number
local function get_x(p)
  return p.x
end

get_x({ x = 1, y = 2 })
";
    let diags = f.check_at(src, Strictness::Strict);
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn shape_struct_in_annotation_checks_fields() {
    let f = geometry_fixture();
    let src = "\
---@use geometry

---@param p Point
local function use(p) end

use({ x = 1 })
";
    let diags = f.check_at(src, Strictness::Strict);
    // The ordinary checker reports the missing field on the literal.
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code.to_string(), "LB0302");
}

// === Resolution (LB2005) ====================================================

#[test]
fn lb2005_unresolved_module_with_p2_note() {
    let f = Fixture::new();
    let diags = f.check("---@use missing_module\n");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code.to_string(), "LB2005");
    assert!(
        diags[0].notes.iter().any(|n| n.contains("P2")),
        "expected the dependency-shapes note: {diags:?}"
    );
}

#[test]
fn sibling_tier_resolves_first() {
    let mut f = Fixture::new();
    f.add_shape_path("shapes");
    // Same module name in both tiers: the sibling must win.
    f.write("src/geometry.lb", "struct Point { x: number }\n");
    f.write(
        "shapes/geometry.lb",
        "struct Point { x: number, y: number }\n",
    );
    // Sibling's Point has no `y`: this literal is clean only via tier 1.
    let diags = f.check("---@use geometry\n\n---@struct Point\nlocal p = { x = 1 }\n");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn shape_path_tier_resolves_when_no_sibling() {
    let mut f = Fixture::new();
    f.add_shape_path("shapes");
    f.write("shapes/geometry.lb", "struct Point { x: number }\n");
    let diags = f.check("---@use geometry\n\n---@struct Point\nlocal p = { x = 1 }\n");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn lb2005_same_tier_ambiguity() {
    let mut f = Fixture::new();
    f.add_shape_path("shapes_a");
    f.add_shape_path("shapes_b");
    f.write("shapes_a/geometry.lb", "struct Point { x: number }\n");
    f.write("shapes_b/geometry.lb", "struct Point { x: number }\n");
    let diags = f.check("---@use geometry\n");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code.to_string(), "LB2005");
    assert!(
        diags[0].message.contains("ambiguous"),
        "{}",
        diags[0].message
    );
}

#[test]
fn use_inside_lb_resolves_transitively() {
    let f = Fixture::new();
    f.write("src/base.lb", "struct Point { x: number, y: number }\n");
    f.write(
        "src/geometry.lb",
        "use base;\nstruct Segment { from: Point, to: Point }\n",
    );
    let src = "\
---@use geometry

---@struct Segment
local s = { from = { x = 0, y = 0 }, to = { x = 1 } }
";
    // The nested `to` literal misses `y` — sealed checking recurses.
    let diags = f.check(src);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code.to_string(), "LB2001");
    assert!(diags[0].message.contains("`y`"));
}

// === Generics (LB2007) ======================================================

#[test]
fn generic_struct_monomorphised_at_binding_site() {
    let f = Fixture::new();
    f.write(
        "src/geometry.lb",
        "struct Pair<T> { first: T, second: T }\n",
    );
    let src = "\
---@use geometry

---@struct Pair<number>
local p = { first = 1, second = 2 }
";
    assert!(f.check(src).is_empty(), "{:?}", f.check(src));

    let bad = "\
---@use geometry

---@struct Pair<number>
local p = { first = 1 }
";
    let diags = f.check(bad);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code.to_string(), "LB2001");
    assert!(diags[0].message.contains("`second`"));
}

#[test]
fn generic_field_types_are_substituted() {
    let f = Fixture::new();
    f.write(
        "src/geometry.lb",
        "struct Pair<T> { first: T, second: T }\n",
    );
    let src = "\
---@use geometry

---@struct Pair<number>
local p = { first = \"no\", second = 2 }
";
    let diags = f.check_at(src, Strictness::Strict);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code.to_string(), "LB0300");
}

#[test]
fn lb2007_bound_violation_at_lua_use_site() {
    let f = Fixture::new();
    f.write(
        "src/geometry.lb",
        "trait Shape {\n    fn area(self) -> number;\n}\n\
         struct Holder<T: Shape> { value: T }\n",
    );
    let src = "\
---@use geometry

---@struct Holder<number>
local h = { value = 1 }
";
    let codes: Vec<String> = f.check(src).iter().map(|d| d.code.to_string()).collect();
    assert_eq!(codes, vec!["LB2007"]);
}

#[test]
fn bound_satisfied_by_lb_impl() {
    let f = Fixture::new();
    f.write(
        "src/geometry.lb",
        "trait Shape {\n    fn area(self) -> number;\n}\n\
         struct Circle { radius: number }\n\
         impl Shape for Circle;\n\
         struct Holder<T: Shape> { value: T }\n",
    );
    let src = "\
---@use geometry

---@struct Holder<Circle>
local h = { value = { radius = 1 } }
";
    assert!(f.check(src).is_empty(), "{:?}", f.check(src));
}

#[test]
fn lb2007_bound_violation_inside_lb_file() {
    let f = Fixture::new();
    f.write(
        "src/geometry.lb",
        "trait Shape {\n    fn area(self) -> number;\n}\n\
         struct Holder<T: Shape> { value: T }\n\
         struct Bad { h: Holder<number> }\n",
    );
    let diags = f.check_lb("src/geometry.lb");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code.to_string(), "LB2007");
    assert_eq!(
        diags[0].primary_label().expect("label").span.file,
        "src/geometry.lb"
    );
}

#[test]
fn vec_and_hashmap_lower_structurally() {
    let f = Fixture::new();
    f.write(
        "src/geometry.lb",
        "struct Poly { points: Vec<number>, tags: HashMap<string, boolean> }\n",
    );
    let src = "\
---@use geometry

---@struct Poly
local p = { points = { 1, 2 }, tags = {} }
";
    assert!(f.check(src).is_empty(), "{:?}", f.check(src));
}

// === Result<T, E> convention (SHAPES.md §12.1) ==============================

#[test]
fn result_expands_to_optional_pair_in_return_position() {
    let f = Fixture::new();
    f.write(
        "src/geometry.lb",
        "struct Point { x: number, y: number }\n\
         trait Parser {\n    fn parse(self, s: string) -> Result<Point, string>;\n}\n",
    );
    let ok = "\
---@use geometry

---@struct Point
local Point = {}
Point.__index = Point

---@impl Parser for Point
---@param s string
---@return Point?, string?
function Point:parse(s)
  return nil, \"unimplemented\"
end
";
    assert!(f.check(ok).is_empty(), "{:?}", f.check(ok));

    // A single un-optional return does not match the (T?, E?) pair.
    let bad = "\
---@use geometry

---@struct Point
local Point = {}
Point.__index = Point

---@impl Parser for Point
---@param s string
---@return string
function Point:parse(s)
  return \"nope\"
end
";
    let codes: Vec<String> = f.check(bad).iter().map(|d| d.code.to_string()).collect();
    assert_eq!(codes, vec!["LB2004"]);
}

// === .lb file checking (LB2010 / syntax) ====================================

#[test]
fn lb2010_body_in_lb_file() {
    let f = Fixture::new();
    f.write(
        "src/bad.lb",
        "trait Shape {\n    fn area(self) -> number { return 1 }\n}\n",
    );
    let diags = f.check_lb("src/bad.lb");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code.to_string(), "LB2010");
    assert!(diags[0].message.contains("implementations live in .lua"));
}

#[test]
fn lb_syntax_error_is_lb0001() {
    let f = Fixture::new();
    f.write("src/bad.lb", "struct { x: number }\n");
    let diags = f.check_lb("src/bad.lb");
    assert!(!diags.is_empty());
    assert!(
        diags.iter().all(|d| d.code.to_string() == "LB0001"),
        "{diags:?}"
    );
}

#[test]
fn lb2005_for_unresolved_use_inside_lb() {
    let f = Fixture::new();
    f.write(
        "src/geometry.lb",
        "use missing;\nstruct Point { x: number }\n",
    );
    let diags = f.check_lb("src/geometry.lb");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code.to_string(), "LB2005");
}

// === Traits as annotation types / zero-cost guarantee =======================

#[test]
fn trait_usable_in_luacats_annotation() {
    let f = circle_fixture();
    // `Shape` must resolve as an annotation type (no LB0305) and expose
    // its method set as a structural interface.
    let src = "\
---@use geometry

---@param s Shape
local function measure(s)
  local area_fn = s.area
  return area_fn
end

measure({ area = function() return 1 end, perimeter = function() return 2 end })
";
    let diags = f.check_at(src, Strictness::Strict);
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn no_tags_means_no_shape_machinery() {
    let f = Fixture::new();
    // No .lb files anywhere; a file without tags must be silent.
    assert!(f.check("local x = 1\nprint(x)\n").is_empty());
}
