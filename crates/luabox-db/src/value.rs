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
use luabox_types::{DisplayTypes, InferredBinding, InferredReturn, TypeEnv};

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

// SAFETY: fully-owned `Arc` payload; replacement via `PartialEq`.
unsafe impl salsa::Update for BindingTypes {
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        // SAFETY: forwarded from the `Update` contract.
        unsafe { replace_if_ne(old_pointer, new_value) }
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
