//! The `LB03xx` diagnostic codes this crate emits — the Semantics block of the
//! `luabox-diag` registry.
//!
//! One `const` per code, so both emission sites and the code *comparisons* on
//! the check path (`---@diagnostic` suppression, cascade collapsing) speak in
//! [`Code`] values. `Code` is a `Copy` newtype over a `u16` with derived
//! equality: comparing two of them is an integer compare, where matching on
//! `code.to_string()` allocated a `String` per diagnostic per check.

use luabox_diag::Code;

/// A value does not fit the type of the slot it flows into.
pub(crate) const TYPE_MISMATCH: Code = Code::new(300);
/// A call passes too few or too many arguments.
pub(crate) const WRONG_ARG_COUNT: Code = Code::new(301);
/// A table literal omits a required `---@field`.
pub(crate) const MISSING_FIELD: Code = Code::new(302);
/// A table literal declares a field its target type does not have.
pub(crate) const UNKNOWN_FIELD: Code = Code::new(303);
/// A `return` value does not fit the declared `---@return`.
pub(crate) const RETURN_MISMATCH: Code = Code::new(304);
/// An annotation names a type that is nowhere declared.
pub(crate) const UNKNOWN_TYPE_NAME: Code = Code::new(305);
/// A read of a provably absent field (luals `undefined-field`).
pub(crate) const FIELD_NOT_FOUND: Code = Code::new(306);
/// One `---@class` name is declared by more than one definition package.
pub(crate) const CLASS_COLLISION: Code = Code::new(307);
/// Use of a `---@deprecated` symbol (luals `deprecated`, #111).
pub(crate) const DEPRECATED: Code = Code::new(308);
/// Discarded return of a `---@nodiscard` call (luals `discard-returns`, #112).
pub(crate) const DISCARD_RETURNS: Code = Code::new(309);
/// One `---@alias` name is declared more than once (luals
/// `duplicate-doc-alias`).
pub(crate) const ALIAS_COLLISION: Code = Code::new(310);
/// Duplicate `---@field` on one class (luals `duplicate-doc-field`).
pub(crate) const DUPLICATE_DOC_FIELD: Code = Code::new(311);
/// Access of a `---@private`/`---@protected`/`---@package` member from outside
/// its visibility scope (luals `invisible`, #115).
pub(crate) const INVISIBLE: Code = Code::new(312);
/// Wrong number of generic type arguments (`Name<A, B>` vs its params, #117).
pub(crate) const GENERIC_ARITY: Code = Code::new(313);
/// A self- or mutually-referential `---@alias` cycle (luals parity, #123).
pub(crate) const CYCLIC_ALIAS: Code = Code::new(314);
/// A non-exhaustive `if`/`elseif` chain dispatching on a finite (literal-union
/// or `---@enum`) discriminant with no `else` branch (#121).
pub(crate) const NON_EXHAUSTIVE_IF: Code = Code::new(315);
/// Call to a `---@async` function from a non-async enclosing function (luals
/// `await-in-sync`).
pub(crate) const AWAIT_IN_SYNC: Code = Code::new(316);
