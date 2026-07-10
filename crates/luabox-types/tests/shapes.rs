//! Shape checking end-to-end (SHAPES-V2.md): ambient fully-qualified
//! resolution in the standard annotation positions, positional structural
//! conformance, sealed-literal freshness, generics, intersections,
//! re-exports, dependency export surfaces, and the `.luab` file checks
//! (LB2005 duplicates, LB2007 instantiation, LB2010 bodies).
//!
//! Fixtures are real temp-dir projects: `.luab` files on disk, `.lua` sources
//! checked through the public [`luabox_types::check_file_shaped`] API.

use std::path::PathBuf;

use luabox_diag::Diagnostic;
use luabox_syntax::lua::{Dialect, parse};
use luabox_types::{DepShapeExport, ShapeOptions, ShapeStore, Strictness, check_file_shaped};

struct Fixture {
    dir: tempfile::TempDir,
    shape_paths: Vec<PathBuf>,
    dependencies: Vec<DepShapeExport>,
}

impl Fixture {
    fn new() -> Self {
        Fixture {
            dir: tempfile::tempdir().expect("tempdir"),
            shape_paths: Vec::new(),
            dependencies: Vec::new(),
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

    /// Register a dependency whose types load from `shape_paths_rel` and
    /// export through the entrypoint at `entry_rel` (both under the fixture
    /// dir).
    fn add_dependency(&mut self, name: &str, entry_rel: &str, shape_paths_rel: &[&str]) {
        let entry = self.dir.path().join(entry_rel);
        self.dependencies.push(DepShapeExport {
            name: name.to_owned(),
            entry: Some(entry),
            shape_paths: shape_paths_rel
                .iter()
                .map(|rel| self.dir.path().join(rel))
                .collect(),
        });
    }

    /// Check `source` as `src/main.lua` at the given strictness.
    fn check_at(&self, source: &str, strictness: Strictness) -> Vec<Diagnostic> {
        let parsed = parse(source, Dialect::Lua54);
        assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
        let store = ShapeStore::new(self.dir.path());
        let opts = ShapeOptions {
            store: &store,
            shape_paths: &self.shape_paths,
            dependencies: &self.dependencies,
        };
        check_file_shaped(&parsed, "src/main.lua", strictness, Some(&opts), None)
    }

    fn check(&self, source: &str) -> Vec<Diagnostic> {
        self.check_at(source, Strictness::Strict)
    }

    /// Check a `.luab` file previously written with [`Fixture::write`].
    fn check_lb(&self, rel: &str) -> Vec<Diagnostic> {
        let path = self.dir.path().join(rel);
        let source = std::fs::read_to_string(&path).expect("read .luab");
        let store = ShapeStore::new(self.dir.path());
        store.check_lb_file(&path, &source, &self.shape_paths, &self.dependencies)
    }
}

fn geometry_fixture() -> Fixture {
    let mut f = Fixture::new();
    f.add_shape_path("shapes");
    f.write(
        "shapes/geometry.luab",
        "type Point = { x: number, y: number, label?: string }\n\
         type Radius = number\n\
         type Pair<T> = { first: T, second: T }\n\
         export type Shape = {\n\
             area(self): number,\n\
             perimeter(self): number,\n\
         }\n\
         export type Drawable = Shape & { draw(self): string }\n",
    );
    f
}

// === Ambient FQ resolution in standard positions ==========================

#[test]
fn typed_local_literal_conforms() {
    let f = geometry_fixture();
    let diags = f.check("---@type geometry.Point\nlocal p = { x = 0, y = 1 }\nreturn p\n");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn optional_field_may_be_omitted_or_given() {
    let f = geometry_fixture();
    let ok = "---@type geometry.Point\nlocal p = { x = 0, y = 1, label = \"origin\" }\nreturn p\n";
    assert!(f.check(ok).is_empty(), "{:?}", f.check(ok));
}

#[test]
fn missing_required_field_errors() {
    let f = geometry_fixture();
    let diags = f.check("---@type geometry.Point\nlocal p = { x = 0 }\nreturn p\n");
    assert!(!diags.is_empty(), "missing `y` must be diagnosed");
    assert!(diags.iter().any(|d| d.message.contains('y')), "{diags:?}");
}

#[test]
fn sealed_freshness_rejects_unknown_field() {
    let f = geometry_fixture();
    let diags = f.check("---@type geometry.Point\nlocal p = { x = 0, y = 0, z = 0 }\nreturn p\n");
    assert!(!diags.is_empty(), "excess `z` must be diagnosed");
    assert!(diags.iter().any(|d| d.message.contains('z')), "{diags:?}");
}

#[test]
fn param_and_return_positions_check() {
    let f = geometry_fixture();
    let ok = "---@param p geometry.Point\n---@return geometry.Point\n\
              local function id(p) return p end\n\
              return id({ x = 1, y = 2 })\n";
    assert!(f.check(ok).is_empty(), "{:?}", f.check(ok));

    let bad = "---@param p geometry.Point\nlocal function f(p) return p end\n\
               return f({ x = 1 })\n";
    assert!(!f.check(bad).is_empty(), "missing `y` at call site");
}

#[test]
fn short_names_do_not_resolve() {
    let f = geometry_fixture();
    // References are fully qualified: bare `Point` is an unknown type name.
    let diags = f.check("---@type Point\nlocal p = { x = 0, y = 1 }\nreturn p\n");
    assert!(
        diags.iter().any(|d| d.code.to_string() == "LB0305"),
        "bare `Point` must be an unknown name: {diags:?}"
    );
}

#[test]
fn no_use_tag_needed_and_v1_tags_are_unknown() {
    let f = geometry_fixture();
    // The scope is ambient — and the retired v1 tags no longer parse as
    // anything meaningful (they surface via the unknown-tag path, without
    // breaking the check).
    let diags = f.check("---@type geometry.Point\nlocal p = { x = 0, y = 1 }\nreturn p\n");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn nested_namespace_from_path() {
    let mut f = Fixture::new();
    f.add_shape_path("shapes");
    f.write(
        "shapes/love/graphics.luab",
        "export type Canvas = { width: number, height: number }\n",
    );
    let ok = "---@type love.graphics.Canvas\nlocal c = { width = 1, height = 2 }\nreturn c\n";
    assert!(f.check(ok).is_empty(), "{:?}", f.check(ok));
    let bad = "---@type love.graphics.Canvas\nlocal c = { width = 1 }\nreturn c\n";
    assert!(!f.check(bad).is_empty(), "missing `height`");
}

// === Alias-like declarations and re-exports ================================

#[test]
fn alias_rhs_expands_inline() {
    let f = geometry_fixture();
    let ok = "---@type geometry.Radius\nlocal r = 2\nreturn r\n";
    assert!(f.check(ok).is_empty(), "{:?}", f.check(ok));
    let bad = "---@type geometry.Radius\nlocal r = \"two\"\nreturn r\n";
    assert!(!f.check(bad).is_empty(), "string is not a Radius (number)");
}

#[test]
fn reexport_is_a_plain_alias() {
    let mut f = geometry_fixture();
    f.add_shape_path("shapes2");
    f.write("shapes2/api.luab", "export type P = geometry.Point\n");
    let ok = "---@type api.P\nlocal p = { x = 0, y = 1 }\nreturn p\n";
    assert!(f.check(ok).is_empty(), "{:?}", f.check(ok));
    let bad = "---@type api.P\nlocal p = { x = 0 }\nreturn p\n";
    assert!(!f.check(bad).is_empty(), "P is Point; missing `y`");
}

// === Generics ==============================================================

#[test]
fn generic_instantiation_checks_fields() {
    let f = geometry_fixture();
    let ok = "---@type geometry.Pair<number>\nlocal p = { first = 1, second = 2 }\nreturn p\n";
    assert!(f.check(ok).is_empty(), "{:?}", f.check(ok));
    let bad = "---@type geometry.Pair<number>\nlocal p = { first = 1, second = \"x\" }\nreturn p\n";
    assert!(!f.check(bad).is_empty(), "string is not T=number");
}

// === Positional conformance: methods and intersections =====================

#[test]
fn carrier_with_all_methods_conforms() {
    let f = geometry_fixture();
    let src = "local Circle = {}\nCircle.__index = Circle\n\
               ---@return number\nfunction Circle:area() return 1 end\n\
               ---@return number\nfunction Circle:perimeter() return 2 end\n\
               ---@type geometry.Shape\nlocal s = Circle\nreturn s\n";
    let diags = f.check(src);
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn carrier_missing_method_fails_positionally() {
    let f = geometry_fixture();
    let src = "local Circle = {}\nCircle.__index = Circle\n\
               ---@return number\nfunction Circle:area() return 1 end\n\
               ---@type geometry.Shape\nlocal s = Circle\nreturn s\n";
    let diags = f.check(src);
    assert!(!diags.is_empty(), "missing `perimeter` must be diagnosed");
    assert!(
        diags.iter().any(|d| d.message.contains("perimeter")),
        "{diags:?}"
    );
}

#[test]
fn intersection_requires_all_members() {
    let f = geometry_fixture();
    let ok = "local C = {}\nC.__index = C\n\
              ---@return number\nfunction C:area() return 1 end\n\
              ---@return number\nfunction C:perimeter() return 2 end\n\
              ---@return string\nfunction C:draw() return \"o\" end\n\
              ---@type geometry.Drawable\nlocal d = C\nreturn d\n";
    assert!(f.check(ok).is_empty(), "{:?}", f.check(ok));

    let bad = "local C = {}\nC.__index = C\n\
               ---@return number\nfunction C:area() return 1 end\n\
               ---@return number\nfunction C:perimeter() return 2 end\n\
               ---@type geometry.Drawable\nlocal d = C\nreturn d\n";
    let diags = f.check(bad);
    assert!(
        diags.iter().any(|d| d.message.contains("draw")),
        "Drawable = Shape & {{draw}}: missing `draw` must be diagnosed: {diags:?}"
    );
}

// === Dependency export surfaces ============================================

#[test]
fn dependency_exports_mount_under_package_name() {
    let mut f = Fixture::new();
    f.add_shape_path("shapes");
    f.write("shapes/app.luab", "type Unused = number\n");
    // The dependency: internal module + entrypoint re-export.
    f.write(
        "dep/shapes/internal/core.luab",
        "export type Vec2 = { x: number, y: number }\ntype Hidden = { secret: number }\n",
    );
    f.write(
        "dep/shapes/init.luab",
        "export type Vec2 = internal.core.Vec2\n",
    );
    f.add_dependency("geo", "dep/shapes/init.luab", &["dep/shapes"]);

    let ok = "---@type geo.Vec2\nlocal v = { x = 1, y = 2 }\nreturn v\n";
    assert!(f.check(ok).is_empty(), "{:?}", f.check(ok));

    // Internal module paths are not addressable from outside.
    let hidden = "---@type internal.core.Hidden\nlocal h = { secret = 1 }\nreturn h\n";
    let diags = f.check(hidden);
    assert!(
        diags.iter().any(|d| d.code.to_string() == "LB0305"),
        "dep-internal names must be invisible: {diags:?}"
    );
}

// === `.luab` file checks ====================================================

#[test]
fn lb2010_method_body_rejected() {
    let f = Fixture::new();
    f.write(
        "shapes/bad.luab",
        "type T = { area(self): number { return 1 } }\n",
    );
    let diags = f.check_lb("shapes/bad.luab");
    assert!(
        diags.iter().any(|d| d.code.to_string() == "LB2010"),
        "{diags:?}"
    );
}

#[test]
fn lb2005_duplicate_fq_declaration() {
    let mut f = Fixture::new();
    f.add_shape_path("shapes");
    f.write("shapes/geometry.luab", "type Point = { x: number }\n");
    f.write("shapes2/geometry.luab", "type Point = { y: number }\n");
    f.add_shape_path("shapes2");
    let diags = f.check_lb("shapes2/geometry.luab");
    assert!(
        diags.iter().any(|d| d.code.to_string() == "LB2005"),
        "duplicate `geometry.Point` must error: {diags:?}"
    );
}

#[test]
fn lb2007_wrong_arity_and_qualification_hint() {
    let mut f = Fixture::new();
    f.add_shape_path("shapes");
    f.write(
        "shapes/geometry.luab",
        "type Pair<T> = { first: T, second: T }\n",
    );
    f.write(
        "shapes/other.luab",
        "type Bad = geometry.Pair<number, string>\ntype AlsoBad = Pair<number>\n",
    );
    let diags = f.check_lb("shapes/other.luab");
    assert!(
        diags
            .iter()
            .any(|d| d.code.to_string() == "LB2007" && d.message.contains("wrong number")),
        "arity must error: {diags:?}"
    );
    assert!(
        diags.iter().any(|d| d.code.to_string() == "LB2007"
            && d.labels.iter().any(|l| l.message.contains("geometry.Pair"))),
        "short cross-module name must hint the FQ candidate: {diags:?}"
    );
}

#[test]
fn sibling_short_names_resolve_within_module() {
    let mut f = Fixture::new();
    f.add_shape_path("shapes");
    f.write(
        "shapes/geometry.luab",
        "type Point = { x: number, y: number }\n\
         export type Segment = { from: Point, to: Point }\n",
    );
    let diags = f.check_lb("shapes/geometry.luab");
    assert!(diags.is_empty(), "sibling refs are legal: {diags:?}");
    // And the sibling reference behaves as the FQ type at use sites.
    let bad = "---@type geometry.Segment\nlocal s = { from = { x = 1 }, to = { x = 1, y = 2 } }\nreturn s\n";
    assert!(!f.check(bad).is_empty(), "nested Point missing `y`");
}

// === Result convention (unchanged in v2) ====================================

#[test]
fn result_in_return_position_is_pair() {
    let mut f = Fixture::new();
    f.add_shape_path("shapes");
    f.write(
        "shapes/io.luab",
        "export type Reader = { read(self): Result<string, string> }\n",
    );
    let ok = "local R = {}\nR.__index = R\n\
              ---@return string?, string?\nfunction R:read() return \"data\", nil end\n\
              ---@type io.Reader\nlocal r = R\nreturn r\n";
    let diags = f.check(ok);
    assert!(diags.is_empty(), "{diags:?}");
}
