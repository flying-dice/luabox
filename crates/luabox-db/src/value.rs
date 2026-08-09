//! Salsa-compatible value wrappers for the producer outputs we memoize.
//!
//! Salsa stores every tracked-query result and needs each stored type to be
//! `'static + Send + Sync + Update` (and, for backdating, `PartialEq`). The
//! upstream producer outputs do not all satisfy those bounds directly:
//!
//! - [`luabox_syntax::lua::Parse`] holds a rowan `GreenNode` (cheap-clone,
//!   `Arc`-backed) plus `Vec<ParseError>`; it is not `PartialEq` and not
//!   `Update`.
//! - [`luabox_types::TypeEnv`] is neither `Clone`, `PartialEq`, nor `Update`.
//!
//! Each wrapper here holds its payload behind an [`Arc`] (so cloning a memo
//! result is a refcount bump) and implements [`salsa::Update`] with the
//! standard "replace if not equal" semantics. Where the payload supports
//! structural equality we also implement [`PartialEq`]/[`Eq`] so salsa can
//! *backdate* — treat a recomputed-but-identical result as unchanged and stop
//! the invalidation there (the firewall). [`TypeEnvHandle`] cannot compare its
//! payload, so its query opts out of backdating with `no_eq` and compares by
//! `Arc` identity in [`salsa::Update`].

use std::sync::Arc;

use luabox_diag::Diagnostic;
use luabox_hir::LoweredFile;
use luabox_syntax::lua;
use luabox_syntax::luacats::AnnotatedItem;
use luabox_types::ty::Ty;
use luabox_types::{Ambient, DisplayTypes, InferredBinding, InferredReturn, TypeEnv};

/// Replace `*old` with `new` when they differ, reporting whether it changed.
///
/// This is the same behaviour salsa's own `update_fallback` provides for
/// `PartialEq` types; we inline it because that helper is a private plumbing
/// detail.
///
/// # Safety
///
/// `old_pointer` must be valid for reads and writes and properly aligned, per
/// the [`salsa::Update`] contract.
unsafe fn replace_if_ne<T: PartialEq>(old_pointer: *mut T, new_value: T) -> bool {
    // SAFETY: the caller upholds the `Update::maybe_update` contract.
    let old = unsafe { &mut *old_pointer };
    if *old == new_value {
        false
    } else {
        *old = new_value;
        true
    }
}

/// The memoized result of parsing one file: a shared, cheap-to-clone handle
/// over [`luabox_syntax::lua::Parse`].
#[derive(Clone, Debug)]
pub struct ParsedModule(Arc<lua::Parse>);

impl ParsedModule {
    pub(crate) fn new(parse: lua::Parse) -> Self {
        Self(Arc::new(parse))
    }

    /// The underlying lossless parse (green tree + parse errors).
    #[must_use]
    pub fn parse(&self) -> &lua::Parse {
        &self.0
    }

    /// The root syntax node — the parse-tree access the LSP reads for a file.
    #[must_use]
    pub fn syntax(&self) -> lua::SyntaxNode {
        self.0.syntax()
    }

    /// The recovered parse errors (the tree is always well-formed regardless).
    #[must_use]
    pub fn errors(&self) -> &[lua::ParseError] {
        self.0.errors()
    }
}

impl PartialEq for ParsedModule {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
            || (self.0.green() == other.0.green() && self.0.errors() == other.0.errors())
    }
}

impl Eq for ParsedModule {}

// SAFETY: the payload is fully owned behind an `Arc`; `maybe_update` fulfils
// the postconditions via `PartialEq` replacement (see `replace_if_ne`).
unsafe impl salsa::Update for ParsedModule {
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        // SAFETY: forwarded from the `Update` contract.
        unsafe { replace_if_ne(old_pointer, new_value) }
    }
}

/// The memoized LuaCATS harvest for one file (the `---@` annotation blocks).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Annotations(Arc<Vec<AnnotatedItem>>);

impl Annotations {
    pub(crate) fn new(items: Vec<AnnotatedItem>) -> Self {
        Self(Arc::new(items))
    }

    /// The harvested annotation blocks with their target statement ranges.
    #[must_use]
    pub fn items(&self) -> &[AnnotatedItem] {
        &self.0
    }
}

// SAFETY: fully-owned `Arc` payload; replacement via `PartialEq`.
unsafe impl salsa::Update for Annotations {
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        // SAFETY: forwarded from the `Update` contract.
        unsafe { replace_if_ne(old_pointer, new_value) }
    }
}

/// The memoized diagnostics for one file (or the aggregated project set).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Diagnostics(Arc<Vec<Diagnostic>>);

impl Diagnostics {
    pub(crate) fn new(diagnostics: Vec<Diagnostic>) -> Self {
        Self(Arc::new(diagnostics))
    }

    /// The diagnostics, in production order.
    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.0
    }

    /// Clone the diagnostics out into an owned `Vec`.
    #[must_use]
    pub fn to_vec(&self) -> Vec<Diagnostic> {
        (*self.0).clone()
    }
}

// SAFETY: fully-owned `Arc` payload; replacement via `PartialEq`.
unsafe impl salsa::Update for Diagnostics {
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        // SAFETY: forwarded from the `Update` contract.
        unsafe { replace_if_ne(old_pointer, new_value) }
    }
}

/// The memoized display-mode inference for one file — the LSP inlay-hint
/// surface ([`luabox_types::infer_display_types`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingTypes(Arc<DisplayTypes>);

impl BindingTypes {
    pub(crate) fn new(types: DisplayTypes) -> Self {
        Self(Arc::new(types))
    }

    /// Every binding's final inferred type, in declaration order.
    #[must_use]
    pub fn bindings(&self) -> &[InferredBinding] {
        &self.0.bindings
    }

    /// Inferred returns per unannotated function, keyed by source range.
    #[must_use]
    pub fn fn_returns(&self) -> &[InferredReturn] {
        &self.0.returns
    }
}

/// The memoized **check-mode** module surface of one file (#85): the
/// `require`-export type a consumer's `require` of this module evaluates
/// to for type checking, plus the file's workspace-global
/// `---@class`/`---@enum` declarations (luals parity: classes declared in
/// any checked file, including their member attachments, resolve from
/// every other file). Computed standalone (the file's own requires are not
/// followed), so the cross-file query graph stays acyclic even when
/// modules require each other.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleSurfaceChecked(Arc<luabox_types::ModuleSurface>);

impl ModuleSurfaceChecked {
    pub(crate) fn new(surface: luabox_types::ModuleSurface) -> Self {
        Self(Arc::new(surface))
    }

    /// The check-mode export type, when the chunk returns a value.
    #[must_use]
    pub fn export(&self) -> Option<&Ty> {
        self.0.export.as_ref()
    }

    /// The file's workspace-global class/enum declarations.
    #[must_use]
    pub fn types(&self) -> &luabox_types::FileTypes {
        &self.0.types
    }
}

// SAFETY: fully-owned `Arc` payload; replacement via `PartialEq`.
unsafe impl salsa::Update for ModuleSurfaceChecked {
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        // SAFETY: forwarded from the `Update` contract.
        unsafe { replace_if_ne(old_pointer, new_value) }
    }
}

/// The memoized inferred module export of one file — what a dependent
/// file's `require` of this module evaluates to. Computed standalone (the
/// file's own requires are not followed), so the cross-file query graph
/// stays acyclic even when modules require each other.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleExport(Arc<Option<Ty>>);

impl ModuleExport {
    pub(crate) fn new(ty: Option<Ty>) -> Self {
        Self(Arc::new(ty))
    }

    /// The inferred export type, when the chunk returns a value.
    #[must_use]
    pub fn ty(&self) -> Option<&Ty> {
        self.0.as_ref().as_ref()
    }
}

// SAFETY: fully-owned `Arc` payload; replacement via `PartialEq`.
unsafe impl salsa::Update for ModuleExport {
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        // SAFETY: forwarded from the `Update` contract.
        unsafe { replace_if_ne(old_pointer, new_value) }
    }
}

/// The memoized outgoing-call arguments of one file: what it passes to
/// functions it does not define, keyed by terminal callee name — the
/// parameter seeds it contributes to the files it requires.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutgoingCalls(Arc<std::collections::HashMap<String, Vec<Ty>>>);

impl OutgoingCalls {
    pub(crate) fn new(calls: std::collections::HashMap<String, Vec<Ty>>) -> Self {
        Self(Arc::new(calls))
    }

    /// Callee name → positional argument-type unions.
    #[must_use]
    pub fn calls(&self) -> &std::collections::HashMap<String, Vec<Ty>> {
        &self.0
    }
}

// SAFETY: fully-owned `Arc` payload; replacement via `PartialEq`.
unsafe impl salsa::Update for OutgoingCalls {
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        // SAFETY: forwarded from the `Update` contract.
        unsafe { replace_if_ne(old_pointer, new_value) }
    }
}

/// The memoized project-wide class/enum contribution merge (#85, round 4
/// review R14): every project file's [`luabox_types::FileTypes`], filtered
/// to the files that declare anything.
///
/// Before this wrapper, `project_types_checked` was a plain function, not a
/// tracked salsa query — and it has three per-file callers
/// (`module_export`, `binding_types`, `module_export_checked`), each itself
/// a tracked query keyed on `(file, project)`. A display pass over an
/// N-file project calls one of those N times, and each call rebuilt this
/// `Vec<FileTypes>` (and paid the `with_project_types` merge over it) from
/// scratch — O(N) work per file, O(N²) total. Making the query itself
/// tracked collapses that to O(N): the merge runs once per project
/// revision and every per-file caller shares the memo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectTypes(Arc<Vec<luabox_types::FileTypes>>);

impl ProjectTypes {
    pub(crate) fn new(types: Vec<luabox_types::FileTypes>) -> Self {
        Self(Arc::new(types))
    }

    /// Every project file's workspace-global class/enum contribution.
    #[must_use]
    pub fn types(&self) -> &[luabox_types::FileTypes] {
        &self.0
    }
}

/// Lets a caller hold or pass `ProjectTypes` exactly where it used to hold
/// or pass `&[FileTypes]` — `&project_types` deref-coerces straight through
/// (round 4 review finding 2: `Host::project_types()` used to hand back a
/// fresh `Vec<FileTypes>` built by `.to_vec()`ing this same slice, deep
/// cloning every file's class/enum/alias maps on every call. `ProjectTypes`
/// is already `Arc`-backed and cheap to clone; this `Deref` is what lets
/// `Host::project_types()` return the wrapper itself — a refcount bump —
/// with no source change at any of its call sites (`MergedAmbient::build`'s
/// `&[FileTypes]` parameter, `.iter()`, `with_project_types`'s
/// `IntoIterator`, ...).
impl std::ops::Deref for ProjectTypes {
    type Target = [luabox_types::FileTypes];

    fn deref(&self) -> &Self::Target {
        self.types()
    }
}

// SAFETY: fully-owned `Arc` payload; replacement via `PartialEq`.
unsafe impl salsa::Update for ProjectTypes {
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        // SAFETY: forwarded from the `Update` contract.
        unsafe { replace_if_ne(old_pointer, new_value) }
    }
}

// SAFETY: fully-owned `Arc` payload; replacement via `PartialEq`.
unsafe impl salsa::Update for BindingTypes {
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        // SAFETY: forwarded from the `Update` contract.
        unsafe { replace_if_ne(old_pointer, new_value) }
    }
}

/// The memoized project-merged ambient layer for one `(project revision,
/// dialect)` (M18, round 6 review): [`luabox_types::Ambient::with_project_types`]
/// applied to [`ProjectTypes`] — the O(N) merge `module_export`/
/// `binding_types`/`module_export_checked` each used to redo from scratch
/// on **every one** of their N per-file calls in a full-project display
/// pass, even though [`project_types_checked`](crate::query::project_types_checked)
/// (the collection the merge runs over) was itself already memoized —
/// O(N) work per call, O(N²) total. This wrapper's own tracked query
/// (`project_ambient` in `query.rs`) makes the merge itself the memoized
/// unit: it runs once per `(project revision, dialect)` and every per-file
/// caller shares the `Arc`.
///
/// [`Ambient`] wraps a [`TypeEnv`] and is not comparable (see
/// [`TypeEnvHandle`]'s doc for the same shape), so this handle compares by
/// `Arc` identity and its query opts out of backdating.
#[derive(Clone, Debug)]
pub struct ProjectAmbient(Arc<Ambient>);

impl ProjectAmbient {
    pub(crate) fn new(ambient: Ambient) -> Self {
        Self(Arc::new(ambient))
    }

    /// The merged ambient layer: dialect stdlib + `[types] defs` (the
    /// caller's `base`) with every project file's workspace-global classes/
    /// enums folded in.
    #[must_use]
    pub fn ambient(&self) -> &Ambient {
        &self.0
    }
}

// SAFETY: fully-owned `Arc` payload. `Ambient` is not `PartialEq` (it wraps
// a `TypeEnv`), so we fall back to `Arc`-identity comparison, exactly like
// [`TypeEnvHandle`]: distinct allocations always replace.
unsafe impl salsa::Update for ProjectAmbient {
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        // SAFETY: forwarded from the `Update` contract.
        let old = unsafe { &mut *old_pointer };
        if Arc::ptr_eq(&old.0, &new_value.0) {
            false
        } else {
            *old = new_value;
            true
        }
    }
}

/// The memoized per-file type environment.
///
/// [`TypeEnv`] is not comparable, so the `type_env` query opts out of
/// backdating (`no_eq`) and this handle compares by `Arc` identity: a fresh
/// build is always considered a change, which is correct if pessimistic.
#[derive(Clone, Debug)]
pub struct TypeEnvHandle(Arc<TypeEnv>);

impl TypeEnvHandle {
    pub(crate) fn new(env: TypeEnv) -> Self {
        Self(Arc::new(env))
    }

    /// The declarations harvested from this file's annotations.
    #[must_use]
    pub fn env(&self) -> &TypeEnv {
        &self.0
    }
}

// SAFETY: fully-owned `Arc` payload. `TypeEnv` is not `PartialEq`, so we fall
// back to `Arc`-identity comparison; distinct allocations always replace.
unsafe impl salsa::Update for TypeEnvHandle {
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        // SAFETY: forwarded from the `Update` contract.
        let old = unsafe { &mut *old_pointer };
        if Arc::ptr_eq(&old.0, &new_value.0) {
            false
        } else {
            *old = new_value;
            true
        }
    }
}

/// The memoized HIR lowering of one file (desugared bodies, name resolution,
/// source map, `require` edges) — what the LSP's goto-definition reads.
///
/// [`LoweredFile`] is not comparable, so the `lower` query opts out of
/// backdating (`no_eq`) and this handle compares by `Arc` identity, exactly
/// like [`TypeEnvHandle`].
#[derive(Clone, Debug)]
pub struct LoweredHandle(Arc<LoweredFile>);

impl LoweredHandle {
    pub(crate) fn new(file: LoweredFile) -> Self {
        Self(Arc::new(file))
    }

    /// The lowered file: bodies, bindings, resolutions, source map, requires.
    #[must_use]
    pub fn file(&self) -> &LoweredFile {
        &self.0
    }
}

// SAFETY: fully-owned `Arc` payload. `LoweredFile` is not `PartialEq`, so we
// fall back to `Arc`-identity comparison; distinct allocations always replace.
unsafe impl salsa::Update for LoweredHandle {
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        // SAFETY: forwarded from the `Update` contract.
        let old = unsafe { &mut *old_pointer };
        if Arc::ptr_eq(&old.0, &new_value.0) {
            false
        } else {
            *old = new_value;
            true
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code — panics document assumptions"
)]
mod tests {
    use std::collections::HashMap;

    use luabox_diag::Severity;
    use luabox_syntax::lua::Dialect;
    use luabox_syntax::luacats;
    use luabox_types::{
        DisplayTypes, ModuleSurface, Strictness, check_file, infer_display_types, module_surface,
    };

    use super::*;

    const CLEAN: &str = "local x = 1\nreturn x\n";
    const OTHER: &str = "local y = 2\nreturn y\n";
    const BAD: &str = "---@param n number\nlocal function f(n) end\nf(\"no\")\n";

    fn parsed(src: &str) -> lua::Parse {
        lua::parse(src, Dialect::Lua54)
    }

    /// Drive [`salsa::Update::maybe_update`] the way salsa's memo table does:
    /// hand it a pointer to the stored value and the freshly recomputed one.
    /// Returns salsa's "did this change?" answer — `false` means backdate.
    fn maybe_update<T: salsa::Update>(stored: &mut T, recomputed: T) -> bool {
        // SAFETY: `stored` is a live, aligned, exclusive reference, which is
        // exactly the `old_pointer` precondition of the `Update` contract.
        unsafe { T::maybe_update(std::ptr::from_mut(stored), recomputed) }
    }

    // --- ParsedModule ------------------------------------------------------

    #[test]
    fn parsed_module_exposes_the_tree_and_the_recovered_errors() {
        let module = ParsedModule::new(parsed(CLEAN));
        assert_eq!(module.errors(), &[]);
        assert_eq!(module.syntax().text().to_string(), CLEAN);
        assert_eq!(module.parse().errors(), module.errors());

        // A broken file still yields a tree; the errors ride alongside it.
        let broken = ParsedModule::new(parsed("local = "));
        assert!(!broken.errors().is_empty());
        assert_eq!(broken.syntax().text().to_string(), "local = ");
    }

    #[test]
    fn two_independent_parses_of_the_same_text_compare_equal() {
        // This is what lets salsa backdate a re-parse of unchanged text: the
        // green trees and error lists match even though the `Arc`s differ.
        let a = ParsedModule::new(parsed(CLEAN));
        let b = ParsedModule::new(parsed(CLEAN));
        assert!(!Arc::ptr_eq(&a.0, &b.0), "distinct allocations");
        assert_eq!(a, b);

        assert_ne!(a, ParsedModule::new(parsed(OTHER)));
        // Same tree shape, different error list: still not equal.
        assert_ne!(
            ParsedModule::new(parsed("local = ")),
            ParsedModule::new(parsed(CLEAN))
        );
    }

    #[test]
    fn parsed_module_backdates_an_identical_reparse_and_replaces_a_changed_one() {
        let mut stored = ParsedModule::new(parsed(CLEAN));
        assert!(
            !maybe_update(&mut stored, ParsedModule::new(parsed(CLEAN))),
            "an identical re-parse must report no change (the firewall)"
        );
        assert_eq!(stored.syntax().text().to_string(), CLEAN);

        assert!(maybe_update(&mut stored, ParsedModule::new(parsed(OTHER))));
        assert_eq!(stored.syntax().text().to_string(), OTHER);
    }

    // --- Annotations -------------------------------------------------------

    #[test]
    fn annotations_hold_the_harvest_and_backdate_on_an_equal_reharvest() {
        let items = luacats::harvest(&parsed(BAD));
        assert!(!items.is_empty(), "the fixture carries a ---@param block");

        let handle = Annotations::new(items.clone());
        assert_eq!(handle.items(), items.as_slice());

        let mut stored = Annotations::new(items);
        assert!(!maybe_update(
            &mut stored,
            Annotations::new(luacats::harvest(&parsed(BAD)))
        ));
        assert!(maybe_update(&mut stored, Annotations::new(Vec::new())));
        assert_eq!(stored.items(), &[]);
    }

    // --- Diagnostics -------------------------------------------------------

    #[test]
    fn diagnostics_default_is_empty_and_to_vec_clones_out() {
        let empty = Diagnostics::default();
        assert_eq!(empty.diagnostics(), &[]);
        assert_eq!(empty.to_vec(), Vec::new());

        let produced = check_file(&parsed(BAD), "a.lua", Strictness::Strict, Dialect::Lua54);
        assert_eq!(produced.len(), 1);
        let handle = Diagnostics::new(produced.clone());
        assert_eq!(handle.diagnostics(), produced.as_slice());
        assert_eq!(handle.to_vec(), produced);
        assert_eq!(handle.diagnostics()[0].severity, Severity::Error);
    }

    #[test]
    fn diagnostics_backdate_when_a_recheck_produces_the_same_set() {
        let recheck = || {
            Diagnostics::new(check_file(
                &parsed(BAD),
                "a.lua",
                Strictness::Strict,
                Dialect::Lua54,
            ))
        };

        let mut stored = recheck();
        assert!(
            !maybe_update(&mut stored, recheck()),
            "an unchanged diagnostic set must not invalidate dependents"
        );
        assert!(maybe_update(&mut stored, Diagnostics::default()));
        assert_eq!(stored.diagnostics(), &[]);
    }

    // --- BindingTypes / ModuleExport / OutgoingCalls / ModuleSurface -------

    #[test]
    fn binding_types_expose_bindings_and_returns_separately() {
        let types = infer_display_types(
            &parsed("local function f() return 1 end\n"),
            "a.lua",
            None,
            None,
        );
        assert!(!types.bindings.is_empty());
        assert!(!types.returns.is_empty());

        let handle = BindingTypes::new(types.clone());
        assert_eq!(handle.bindings(), types.bindings.as_slice());
        assert_eq!(handle.fn_returns(), types.returns.as_slice());

        let mut stored = BindingTypes::new(types);
        assert!(!maybe_update(
            &mut stored,
            BindingTypes::new(infer_display_types(
                &parsed("local function f() return 1 end\n"),
                "a.lua",
                None,
                None
            ))
        ));
        assert!(maybe_update(
            &mut stored,
            BindingTypes::new(DisplayTypes::default())
        ));
        assert_eq!(stored.bindings(), &[]);
        assert_eq!(stored.fn_returns(), &[]);
    }

    #[test]
    fn module_export_is_none_for_a_chunk_with_no_return_value() {
        let exporting =
            infer_display_types(&parsed("return 42\n"), "m.lua", None, None).module_export;
        assert!(exporting.is_some(), "`return 42` exports a type");

        assert_eq!(ModuleExport::new(None).ty(), None);
        let handle = ModuleExport::new(exporting.clone());
        assert_eq!(handle.ty(), exporting.as_ref());

        let mut stored = ModuleExport::new(exporting.clone());
        assert!(!maybe_update(&mut stored, ModuleExport::new(exporting)));
        assert!(maybe_update(&mut stored, ModuleExport::new(None)));
        assert_eq!(stored.ty(), None);
    }

    #[test]
    fn outgoing_calls_key_argument_types_by_callee_name() {
        let calls = infer_display_types(&parsed("undefined_helper(1)\n"), "a.lua", None, None)
            .outgoing_calls;
        assert!(
            calls.contains_key("undefined_helper"),
            "an undefined callee is recorded as an outgoing call: {calls:?}"
        );

        let handle = OutgoingCalls::new(calls.clone());
        assert_eq!(handle.calls(), &calls);

        let mut stored = OutgoingCalls::new(calls);
        assert!(!maybe_update(
            &mut stored,
            OutgoingCalls::new(
                infer_display_types(&parsed("undefined_helper(1)\n"), "a.lua", None, None)
                    .outgoing_calls
            )
        ));
        assert!(maybe_update(
            &mut stored,
            OutgoingCalls::new(HashMap::new())
        ));
        assert!(stored.calls().is_empty());
    }

    #[test]
    fn module_surface_checked_splits_the_export_from_the_declared_types() {
        let surface = module_surface(
            &parsed("---@class Point\nlocal M = {}\nreturn M\n"),
            "m.lua",
            None,
        );
        assert!(surface.export.is_some());

        let handle = ModuleSurfaceChecked::new(surface.clone());
        assert_eq!(handle.export(), surface.export.as_ref());
        assert_eq!(handle.types(), &surface.types);

        let mut stored = ModuleSurfaceChecked::new(surface);
        assert!(!maybe_update(
            &mut stored,
            ModuleSurfaceChecked::new(module_surface(
                &parsed("---@class Point\nlocal M = {}\nreturn M\n"),
                "m.lua",
                None
            ))
        ));
        assert!(maybe_update(
            &mut stored,
            ModuleSurfaceChecked::new(ModuleSurface::default())
        ));
        assert_eq!(stored.export(), None);
    }

    // --- the two no_eq, Arc-identity handles -------------------------------

    #[test]
    fn type_env_handle_compares_by_arc_identity_not_by_content() {
        let handle = TypeEnvHandle::new(TypeEnv::build(&parsed(BAD)));
        // The handle hands back the env the harvest produced.
        assert_eq!(
            format!("{:?}", handle.env()),
            format!("{:?}", TypeEnv::build(&parsed(BAD)))
        );

        // Cloning shares the `Arc`, so salsa sees no change…
        let mut stored = handle.clone();
        assert!(!maybe_update(&mut stored, handle));

        // …but a freshly built env over the *same* text is a distinct
        // allocation, and `TypeEnv` cannot be compared, so it always replaces.
        assert!(maybe_update(
            &mut stored,
            TypeEnvHandle::new(TypeEnv::build(&parsed(BAD)))
        ));
    }

    #[test]
    fn lowered_handle_compares_by_arc_identity_not_by_content() {
        let handle = LoweredHandle::new(luabox_hir::lower(&parsed(CLEAN)));
        assert_eq!(handle.file().bodies().count(), 1, "just the chunk");

        let mut stored = handle.clone();
        assert!(!maybe_update(&mut stored, handle));

        assert!(maybe_update(
            &mut stored,
            LoweredHandle::new(luabox_hir::lower(&parsed(CLEAN)))
        ));
    }
}
