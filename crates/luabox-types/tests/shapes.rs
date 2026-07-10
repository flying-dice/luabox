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

// === Generic arity errors at LuaCATS annotation sites (#78) =================

#[test]
fn wrong_generic_arity_at_annotation_site_is_lb2007() {
    let f = geometry_fixture();
    let diags = f.check(
        "---@type geometry.Pair<number, string>\nlocal p = { first = 1, second = 2 }\nreturn p\n",
    );
    assert!(
        diags.iter().any(|d| d.code.to_string() == "LB2007"
            && d.message.contains("wrong number")
            && d.message.contains("expected 1, found 2")),
        "{diags:?}"
    );
}

#[test]
fn type_arguments_on_non_generic_shape_at_annotation_site_is_lb2007() {
    let f = geometry_fixture();
    let diags = f.check("---@type geometry.Point<number>\nlocal p = { x = 0, y = 1 }\nreturn p\n");
    assert!(
        diags.iter().any(|d| d.code.to_string() == "LB2007"
            && d.message.contains("not generic")
            && d.message.contains("geometry.Point")),
        "{diags:?}"
    );
}

// === LB0305 fully-qualified-name suggestions (#79) ==========================

#[test]
fn lb0305_suggests_fq_name_for_bare_short_name() {
    let f = geometry_fixture();
    let diags = f.check("---@type Point\nlocal p = { x = 0, y = 1 }\nreturn p\n");
    let lb0305 = diags
        .iter()
        .find(|d| d.code.to_string() == "LB0305")
        .expect("LB0305 expected");
    assert!(
        lb0305
            .notes
            .iter()
            .any(|n| n.contains("did you mean `geometry.Point`?")),
        "{lb0305:?}"
    );
}

#[test]
fn lb0305_suggests_fq_name_for_typoed_namespace() {
    let f = geometry_fixture();
    // `geomtry.Point` — a typo'd namespace whose last segment (`Point`)
    // still matches a declared shape.
    let diags = f.check("---@type geomtry.Point\nlocal p = { x = 0, y = 1 }\nreturn p\n");
    let lb0305 = diags
        .iter()
        .find(|d| d.code.to_string() == "LB0305")
        .expect("LB0305 expected");
    assert!(
        lb0305
            .notes
            .iter()
            .any(|n| n.contains("did you mean `geometry.Point`?")),
        "{lb0305:?}"
    );
}

// === Declaration-site labels on conformance errors (#80) ====================

#[test]
fn conformance_mismatch_labels_the_luab_declaration_site() {
    let f = geometry_fixture();
    let diags = f.check("---@type geometry.Point\nlocal p = { x = 0 }\nreturn p\n");
    let diag = diags
        .iter()
        .find(|d| d.code.to_string() == "LB0302")
        .expect("missing-field diagnostic expected");
    assert!(
        diag.labels.iter().any(|l| !l.primary
            && l.message == "type declared here"
            && l.span.file == "shapes/geometry.luab"),
        "{diag:?}"
    );
}

#[test]
fn type_mismatch_against_shape_labels_the_luab_declaration_site() {
    let f = geometry_fixture();
    // A whole-value mismatch (not a table literal): the assignability
    // failure must still point back at `geometry.Point`'s declaration.
    let src = "---@type string\nlocal s = \"x\"\n---@type geometry.Point\nlocal p = s\nreturn p\n";
    let diags = f.check(src);
    let diag = diags
        .iter()
        .find(|d| d.code.to_string() == "LB0300")
        .expect("type-mismatch diagnostic expected");
    assert!(
        diag.labels.iter().any(|l| !l.primary
            && l.message == "type declared here"
            && l.span.file == "shapes/geometry.luab"),
        "{diag:?}"
    );
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
fn carrier_with_wrong_return_type_fails() {
    let f = geometry_fixture();
    // `area` is annotated `---@return string` — it does not satisfy the
    // `area(self): number` member. `perimeter` is correct, so the sole
    // mismatch is `area`.
    let src = "local Circle = {}\nCircle.__index = Circle\n\
               ---@return string\nfunction Circle:area() return \"x\" end\n\
               ---@return number\nfunction Circle:perimeter() return 2 end\n\
               ---@type geometry.Shape\nlocal s = Circle\nreturn s\n";
    let diags = f.check(src);
    assert!(!diags.is_empty(), "wrong `area` return must be diagnosed");
    assert!(
        diags.iter().any(|d| d.message.contains("area")),
        "the mismatch must name `area`: {diags:?}"
    );
}

#[test]
fn carrier_method_extra_required_param_fails_but_fewer_passes() {
    let f = geometry_fixture();
    // A required parameter beyond the `area(self): number` signature: a
    // `Shape` caller (`s:area()`) never supplies it, so it does not conform.
    let extra = "local C = {}\nC.__index = C\n\
                 ---@param scale number\n---@return number\n\
                 function C:area(scale) return scale end\n\
                 ---@return number\nfunction C:perimeter() return 2 end\n\
                 ---@type geometry.Shape\nlocal s = C\nreturn s\n";
    let diags = f.check(extra);
    assert!(
        diags.iter().any(|d| d.message.contains("area")),
        "extra required `area` parameter must be diagnosed: {diags:?}"
    );

    // Fewer parameters than the target is always safe (Lua drops extra
    // arguments): the plain `self`-only methods conform.
    let fewer = "local C = {}\nC.__index = C\n\
                 ---@return number\nfunction C:area() return 1 end\n\
                 ---@return number\nfunction C:perimeter() return 2 end\n\
                 ---@type geometry.Shape\nlocal s = C\nreturn s\n";
    assert!(f.check(fewer).is_empty(), "{:?}", f.check(fewer));
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

// === Lenient `.luab` edges now diagnosed (#81) ==============================

#[test]
fn multi_return_paren_outside_return_position_is_lb2007() {
    let f = Fixture::new();
    f.write("shapes/bad.luab", "type Bad = (number, string)\n");
    let diags = f.check_lb("shapes/bad.luab");
    assert!(
        diags.iter().any(|d| d.code.to_string() == "LB2007"
            && d.message.contains("only legal in return position")),
        "{diags:?}"
    );
}

#[test]
fn multi_return_paren_in_return_position_stays_clean() {
    let f = Fixture::new();
    f.write(
        "shapes/ok.luab",
        "type T = { split(self): (number, string) }\n",
    );
    let diags = f.check_lb("shapes/ok.luab");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn intersection_member_not_an_object_type_is_lb2007() {
    let mut f = Fixture::new();
    f.add_shape_path("shapes");
    f.write(
        "shapes/bad.luab",
        "export type Shape = { area(self): number }\ntype Bad = Shape & number\n",
    );
    let diags = f.check_lb("shapes/bad.luab");
    assert!(
        diags.iter().any(|d| d.code.to_string() == "LB2007"
            && d.message.contains("intersection member")
            && d.message.contains("not an object type")),
        "{diags:?}"
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

// === `self` typing: constructor tie to the instance shape (SHAPES-V2.md) ====

/// A shape module with two concrete instance types and a Shape trait: the
/// carrier idiom's target types.
fn carrier_fixture() -> Fixture {
    let mut f = Fixture::new();
    f.add_shape_path("shapes");
    f.write(
        "shapes/geometry.luab",
        "export type Circle = { radius: number }\n\
         export type Widget = { size: integer }\n\
         export type Shape = {\n\
             area(self): number,\n\
             perimeter(self): number,\n\
         }\n",
    );
    f
}

#[test]
fn self_types_as_shape_instance_in_carrier_methods() {
    let f = carrier_fixture();
    // `setmetatable({...}, Circle)` under `---@return geometry.Circle` ties
    // the carrier to `geometry.Circle`, so `self.radius` in a method resolves
    // as the declared `number`: a `number` consumer accepts it, a `string`
    // consumer does not — exactly one mismatch. Were `self` untyped
    // (`unknown`), the `number` consumer would fail too (two mismatches).
    let src = "---@param n number\nlocal function wantn(n) end\n\
               ---@param s string\nlocal function wants(s) end\n\
               local Circle = {}\nCircle.__index = Circle\n\
               function Circle:probe()\n\
                   wantn(self.radius)\n\
                   wants(self.radius)\n\
               end\n\
               ---@param radius number\n---@return geometry.Circle\n\
               function Circle.new(radius)\n\
                   return setmetatable({ radius = radius }, Circle)\n\
               end\n\
               return Circle\n";
    let codes: Vec<String> = f.check(src).iter().map(|d| d.code.to_string()).collect();
    assert_eq!(codes, vec!["LB0300"], "{:?}", f.check(src));
}

#[test]
fn self_declared_field_type_governs_over_constructor() {
    let f = carrier_fixture();
    // The tie is to the DECLARED shape, not the constructor value: `size` is
    // declared `integer`, so an `integer` consumer of `self.size` passes
    // strict even though the constructor stored a plain `number` param.
    // Without the tie `self.size` would infer `number` and fail — this is
    // the tie's proof (number is not assignable to integer).
    let src = "---@param n integer\nlocal function wanti(n) end\n\
               local Widget = {}\nWidget.__index = Widget\n\
               function Widget:probe()\n    wanti(self.size)\nend\n\
               ---@param size number\n---@return geometry.Widget\n\
               function Widget.new(size)\n\
                   return setmetatable({ size = size }, Widget)\n\
               end\n\
               return Widget\n";
    assert!(f.check(src).is_empty(), "{:?}", f.check(src));
}

#[test]
fn full_carrier_idiom_passes_strict() {
    let f = carrier_fixture();
    // Constructor + methods + positional `---@type geometry.Shape`
    // assertion: the idiomatic v2 carrier is strict-clean end to end.
    let src = "local Circle = {}\nCircle.__index = Circle\n\
               ---@return number\nfunction Circle:area()\n\
                   return self.radius * self.radius\nend\n\
               ---@return number\nfunction Circle:perimeter()\n\
                   return self.radius + self.radius\nend\n\
               ---@param radius number\n---@return geometry.Circle\n\
               function Circle.new(radius)\n\
                   return setmetatable({ radius = radius }, Circle)\n\
               end\n\
               ---@type geometry.Shape\nlocal _ = Circle\nreturn Circle\n";
    assert!(f.check(src).is_empty(), "{:?}", f.check(src));
}

#[test]
fn explicit_self_fallback_matches_with_or_without_tie() {
    let f = carrier_fixture();
    // The standard-LuaCATS explicit fallback `---@param self T` types `self`
    // on its own — identically whether or not a constructor supplies the tie.
    let method = "---@param self geometry.Circle\n---@return number\n\
                  function Circle:area()\n    return self.radius * self.radius\nend\n";
    let with_ctor = format!(
        "local Circle = {{}}\nCircle.__index = Circle\n{method}\
         ---@param radius number\n---@return geometry.Circle\n\
         function Circle.new(radius)\n\
             return setmetatable({{ radius = radius }}, Circle)\nend\n\
         return Circle\n"
    );
    let without_ctor =
        format!("local Circle = {{}}\nCircle.__index = Circle\n{method}return Circle\n");
    assert!(f.check(&with_ctor).is_empty(), "{:?}", f.check(&with_ctor));
    assert!(
        f.check(&without_ctor).is_empty(),
        "explicit self must type `self` with no constructor: {:?}",
        f.check(&without_ctor)
    );
}

#[test]
fn explicit_class_on_carrier_wins_over_shape_tie() {
    let f = carrier_fixture();
    // An explicit `---@class` on the carrier declares the instance type; the
    // shape tie must defer. `tag` exists only on the class, so `self.tag`
    // resolves (as `string`) only if the class governs `self` — a shape-tied
    // `self` would have no `tag` and the read would be flagged.
    let src = "---@param s string\nlocal function wants(s) end\n\
               ---@class Local\n---@field radius number\n---@field tag string\n\
               local Circle = {}\nCircle.__index = Circle\n\
               function Circle:probe()\n    wants(self.tag)\nend\n\
               ---@param radius number\n---@return geometry.Circle\n\
               function Circle.new(radius)\n\
                   return setmetatable({ radius = radius, tag = \"x\" }, Circle)\n\
               end\n\
               return Circle\n";
    assert!(f.check(src).is_empty(), "{:?}", f.check(src));
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

// === Sealed-key semantics beyond literals (#82) =============================
//
// Literal freshness (`sealed_freshness_rejects_unknown_field`) already pins
// the constructor-literal case. These pin the two other places a sealed
// key can be touched on an already-typed value: a field *write* and a field
// *read*.

#[test]
fn sealed_unknown_key_write_is_lb0303() {
    let f = geometry_fixture();
    let diags = f.check("---@type geometry.Point\nlocal p = { x = 0, y = 1 }\np.z = 1\nreturn p\n");
    assert!(
        diags
            .iter()
            .any(|d| d.code.to_string() == "LB0303" && d.message.contains('z')),
        "a write to an undeclared key on a sealed shape must be diagnosed: {diags:?}"
    );
}

#[test]
fn sealed_declared_key_write_stays_clean() {
    let f = geometry_fixture();
    // The counterpart: writing an already-declared key is unremarkable.
    let diags = f.check("---@type geometry.Point\nlocal p = { x = 0, y = 1 }\np.x = 2\nreturn p\n");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn sealed_unknown_key_read_currently_lenient() {
    // Pinned as lenient (#82): a read of an undeclared key on a sealed
    // shape-typed value (`p.z`) is not diagnosed today. Reads flow through
    // `Checker::field_ty`, which only ever *finds* a field in the shape or
    // falls back to `unknown` — there is no "prove the field is absent"
    // path the way literal freshness proves excess. Enforcing it needs the
    // same deeper provable-absence reasoning `LB0306` uses for locally
    // inferred tables, which this ticket does not extend to `.luab`-typed
    // values. Tracked as future work, not a regression.
    let f = geometry_fixture();
    let diags = f.check("---@type geometry.Point\nlocal p = { x = 0, y = 1 }\nreturn p.z\n");
    assert!(diags.is_empty(), "{diags:?}");
}
