//! Lint levels and the effective-configuration resolver (SPEC.md §9).
//!
//! Mirrors clippy's `allow`/`warn`/`deny` ladder. A rule's effective level is
//! its tier default, overridden by a `[lint]` tier toggle, overridden by a
//! `[lint]` rule-id entry — most specific wins.
//!
//! The manifest model for `[lint]` lives in `luabox-manifest`, and the
//! translation onto this crate's vocabulary lives *here*, as [`From`] impls on
//! the re-exported [`LintLevel`]/[`LintTier`]: `luabox-cli` and `luabox-lsp`
//! both build a [`LintConfig`] from a manifest, and each used to carry its own
//! `LintLevel` → level-keyword function that the other had to stay in step
//! with (CC-M8). Still acyclic (SPEC.md §16) — `luabox-manifest` depends on
//! nothing in this workspace.

use std::collections::{HashMap, HashSet};

use luabox_diag::Severity;
pub use luabox_manifest::model::{LintLevel, LintTier};

use crate::rule::{Rule, Tier};

/// A lint level: off, warn, or deny (SPEC.md §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Rule disabled.
    Allow,
    /// Warning severity (does not fail the command).
    Warn,
    /// Error severity (fails the command).
    Deny,
}

impl Level {
    /// The rendered severity, or `None` when the rule is off.
    #[must_use]
    pub fn severity(self) -> Option<Severity> {
        match self {
            Level::Allow => None,
            Level::Warn => Some(Severity::Warning),
            Level::Deny => Some(Severity::Error),
        }
    }

    /// Parse a `[lint]` level keyword.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Level> {
        match raw {
            "allow" => Some(Level::Allow),
            "warn" => Some(Level::Warn),
            "deny" => Some(Level::Deny),
            _ => None,
        }
    }
}

impl From<LintLevel> for Level {
    /// The one `[lint]` level → [`Level`] mapping in the workspace.
    fn from(level: LintLevel) -> Self {
        match level {
            LintLevel::Allow => Level::Allow,
            LintLevel::Warn => Level::Warn,
            LintLevel::Deny => Level::Deny,
        }
    }
}

impl From<LintTier> for Tier {
    /// The one `[lint]` tier → [`Tier`] mapping in the workspace. Exhaustive
    /// on both sides, so a tier added to either vocabulary stops this
    /// compiling rather than silently becoming a no-op override.
    fn from(tier: LintTier) -> Self {
        match tier {
            LintTier::Correctness => Tier::Correctness,
            LintTier::Suspicious => Tier::Suspicious,
            LintTier::Perf => Tier::Perf,
            LintTier::Style => Tier::Style,
            LintTier::Pedantic => Tier::Pedantic,
        }
    }
}

/// The tier default before any `[lint]` override (SPEC.md §9): correctness is
/// `deny`, suspicious/perf/style are `warn`, pedantic is off.
#[must_use]
pub fn tier_default(tier: Tier) -> Level {
    match tier {
        Tier::Correctness => Level::Deny,
        Tier::Suspicious | Tier::Perf | Tier::Style => Level::Warn,
        Tier::Pedantic => Level::Allow,
    }
}

/// Resolved lint configuration for a project.
#[derive(Debug, Clone, Default)]
pub struct LintConfig {
    globals: HashSet<String>,
    tiers: HashMap<Tier, Level>,
    rules: HashMap<String, Level>,
}

impl LintConfig {
    /// An empty configuration: every rule at its tier default.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a name the `global-write` rule should treat as an intended global.
    pub fn allow_global(&mut self, name: impl Into<String>) {
        self.globals.insert(name.into());
    }

    /// Set a tier-level override.
    ///
    /// Typed on both arguments, so there is no unrecognised-name case left to
    /// silently swallow: the string-keyed setter this replaced returned a
    /// `bool` that every caller in the workspace discarded (CC-M8).
    pub fn set_tier(&mut self, tier: Tier, level: Level) {
        self.tiers.insert(tier, level);
    }

    /// Set a rule-id override. The rule id stays a string — ids are open, and
    /// live with the rules themselves — but the level is typed.
    pub fn set_rule(&mut self, rule_id: &str, level: Level) {
        self.rules.insert(rule_id.to_owned(), level);
    }

    /// Whether `name` is on the `global-write` allow-list.
    #[must_use]
    pub fn is_allowed_global(&self, name: &str) -> bool {
        self.globals.contains(name)
    }

    /// The effective level for a rule: tier default → tier override → rule
    /// override.
    #[must_use]
    pub fn effective(&self, rule: &dyn Rule) -> Level {
        let mut level = tier_default(rule.tier());
        if let Some(&tier) = self.tiers.get(&rule.tier()) {
            level = tier;
        }
        if let Some(&over) = self.rules.get(rule.id()) {
            level = over;
        }
        level
    }
}
