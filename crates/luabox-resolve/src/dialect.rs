//! Dialect **family sets** — the resolver's compatibility model
//! (SPEC.md §6, flying-dice/luabox#5).
//!
//! A package declares the Lua dialects it is source-compatible with as an
//! explicit *family set* — **never a range**. A range implies a total order
//! that LuaJIT breaks: LuaJIT is 5.1-plus-extensions, not a point between 5.1
//! and 5.2, so `>= 5.1` as an ordered interval is meaningless for it (#5).
//! The set is the internal resolver model *and* the diagnostic vocabulary; the
//! LuaRocks bridge is the one boundary where ranges come *in* (a rockspec's
//! `lua` constraint) and family sets come *out* (see
//! [`crate::luarocks`]'s `lua_dialects`).
//!
//! # Where a package's set comes from
//!
//! In precedence order, funnelled through
//! [`PackageMeta::lua_versions`](crate::PackageMeta):
//!
//! 1. a registry package's rockspec `lua` constraint, translated to the set of
//!    dialects it admits (`lua-versions` metadata, no `lua` dep → all);
//! 2. a path/git package's `luabox.toml` `[package] lua-versions`;
//! 3. otherwise **absent = unconstrained** = every dialect.
//!
//! # LuaJIT membership
//!
//! A rockspec `lua` constraint admits LuaJIT **iff it admits 5.1**, since
//! LuaJIT is 5.1-family (`lua ~> 5.1` / `lua >= 5.1` is conventionally
//! LuaJIT-satisfiable). That rule lives in the rockspec→set translation
//! ([`crate::luarocks`]); this module treats [`Dialect::LuaJit`] as an ordinary
//! member with no implicit tie to 5.1 (a `luabox.toml` set lists exactly what
//! it lists).
//!
//! # The Luau fence
//!
//! Luau is not a [`Dialect`] variant and is wired nowhere. It can therefore
//! never enter a [`DialectSet`] nor be an argument to [`lowerable`] — which is
//! exactly the fence #5 asks for: a Luau package would have no lowering path to
//! any PUC target. The model stays *open* (a set of dialect ids, extensible)
//! without precluding a future Luau family.

use std::collections::BTreeSet;

use luabox_syntax::Dialect;

/// The set of dialects a package supports. An **empty** set means
/// *unconstrained* — compatible with every dialect (an absent declaration,
/// SPEC.md §6).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DialectSet {
    dialects: BTreeSet<Dialect>,
}

impl DialectSet {
    /// Build a set from `lua-versions`/rockspec-translated dialect ids
    /// (`"5.1"`, `"luajit"`, …). Ids no [`Dialect`] recognises — never emitted
    /// by a validated manifest or the rockspec translation, but possible for a
    /// hypothetical `"luau"` — are dropped: they name a family this resolver
    /// cannot model, so they contribute no membership (and the Luau fence in
    /// [`lowerable`] does the rejecting). An empty result is *unconstrained*.
    pub fn from_ids<I, S>(ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            dialects: ids
                .into_iter()
                .filter_map(|id| Dialect::from_manifest_id(id.as_ref()))
                .collect(),
        }
    }

    /// Whether this set declares nothing, i.e. admits *every* dialect
    /// (SPEC.md §6: an absent `lua-versions` is unconstrained).
    #[must_use]
    pub fn is_unconstrained(&self) -> bool {
        self.dialects.is_empty()
    }

    /// Literal membership: whether `dialect` is one of the declared dialects.
    /// Does **not** treat unconstrained as "contains everything" — callers pair
    /// this with [`Self::is_unconstrained`] (see [`Self::admits`]).
    #[must_use]
    pub fn contains(&self, dialect: Dialect) -> bool {
        self.dialects.contains(&dialect)
    }

    /// Whether the package is *directly* usable for `target`: unconstrained, or
    /// `target` is a declared member. (The resolver adds a lowering escape
    /// hatch on top of this — see [`lowerable`].)
    #[must_use]
    pub fn admits(&self, target: Dialect) -> bool {
        self.is_unconstrained() || self.contains(target)
    }

    /// A stable, human-readable list for diagnostics: `"5.1, 5.2, 5.3"`, or
    /// `"any"` when unconstrained.
    #[must_use]
    pub fn describe(&self) -> String {
        if self.is_unconstrained() {
            return "any".to_owned();
        }
        self.dialects
            .iter()
            .map(|d| d.manifest_id())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Resolve-time lowerability of the `from`→`to` direction (SPEC.md §2.1, §6).
///
/// `luabox-lower` defines a lowering pass for **every** ordered pair within the
/// PUC/LuaJIT dialect family ([`Dialect::ALL`]): downgrades rewrite features
/// (`goto`, `//`, bitops, `<const>`/`<close>`, `_ENV`, LuaJIT `bit`),
/// `from == to` is the identity, and upgrade/cross pairs need no rewrite. A
/// *construct* the rules cannot lower (an irreducible `goto`, `ffi`, …) is a
/// hard **build**-time error caught by residual validation of the lowered
/// output — it is not knowable at resolve time without the sources. So at
/// resolve time any PUC/LuaJIT edition is considered lowerable to any
/// PUC/LuaJIT target, and `build` is trusted to reject the residue.
///
/// This is deliberately a *family-level* gate rather than a per-construct one:
/// resolution has no sources in hand. Its source of truth is `luabox-lower`'s
/// documented support matrix (total over [`Dialect::ALL`]) — chosen over
/// probing `luabox-lower::lower` on an empty file (which trivially succeeds for
/// every pair, so answers nothing) and over adding a `luabox-lower` dependency
/// to this Distribution crate (SPEC.md §16 layering).
///
/// The Luau fence falls out for free: a Luau "edition" never parses to a
/// [`Dialect`], so it can never be a `from` here — no lowering path, ever.
#[must_use]
pub fn lowerable(from: Dialect, to: Dialect) -> bool {
    // Both are members of `Dialect::ALL` by construction; luabox-lower is total
    // over that family. Kept as an explicit predicate so there is a single
    // place to gate the day a direction becomes unsupported (or Luau lowering
    // lands).
    Dialect::ALL.contains(&from) && Dialect::ALL.contains(&to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_declaration_is_unconstrained_and_admits_all() {
        let set = DialectSet::from_ids(Vec::<&str>::new());
        assert!(set.is_unconstrained());
        for d in Dialect::ALL {
            assert!(set.admits(d), "unconstrained set must admit {d:?}");
        }
        assert_eq!(set.describe(), "any");
    }

    #[test]
    fn membership_is_literal_and_described_sorted() {
        // Deliberately out of order to prove the set normalizes.
        let set = DialectSet::from_ids(["5.3", "5.1", "5.2"]);
        assert!(!set.is_unconstrained());
        assert!(set.contains(Dialect::Lua51));
        assert!(set.admits(Dialect::Lua53));
        assert!(!set.contains(Dialect::Lua54));
        assert!(!set.admits(Dialect::Lua54));
        // BTreeSet ordering: Lua51 < Lua52 < Lua53 (declaration order in enum).
        assert_eq!(set.describe(), "5.1, 5.2, 5.3");
    }

    #[test]
    fn luajit_is_an_ordinary_member_not_tied_to_51() {
        // In a `luabox.toml` set, luajit is exactly what is listed — no
        // implicit link to 5.1 (that tie lives only in the rockspec
        // translation).
        let jit_only = DialectSet::from_ids(["luajit"]);
        assert!(jit_only.contains(Dialect::LuaJit));
        assert!(!jit_only.contains(Dialect::Lua51));

        let five_one_only = DialectSet::from_ids(["5.1"]);
        assert!(five_one_only.contains(Dialect::Lua51));
        assert!(!five_one_only.contains(Dialect::LuaJit));
    }

    #[test]
    fn the_luarocks_boundary_set() {
        // `lua >= 5.1, < 5.4` translates to {5.1, 5.2, 5.3, luajit} at the
        // bridge; here we exercise the PUC members the boundary test relies on.
        let set = DialectSet::from_ids(["5.1", "5.2", "5.3", "luajit"]);
        assert!(set.admits(Dialect::Lua51));
        assert!(!set.admits(Dialect::Lua54));
    }

    #[test]
    fn unknown_ids_like_luau_are_dropped() {
        // A "luau" id names a family this resolver cannot model: it contributes
        // no membership. (Manifests never produce it — the fence is structural.)
        let set = DialectSet::from_ids(["luau"]);
        assert!(set.is_unconstrained(), "luau contributes nothing");
    }

    #[test]
    fn lowerable_is_total_over_the_puc_family() {
        for from in Dialect::ALL {
            for to in Dialect::ALL {
                assert!(lowerable(from, to), "{from:?} -> {to:?} must be lowerable");
            }
        }
    }
}
