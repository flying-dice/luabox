//! Lint levels and the effective-configuration resolver (SPEC.md §9).
//!
//! Mirrors clippy's `allow`/`warn`/`deny` ladder. A rule's effective level is
//! its tier default, overridden by a `[lint]` tier toggle, overridden by a
//! `[lint]` rule-id entry — most specific wins. The manifest model for
//! `[lint]` lives in `luabox-resolve`; this crate is fed the already-parsed
//! values (the Frontend translates them) so the Semantics/Frontend layering
//! stays acyclic (SPEC.md §16).

use std::collections::{HashMap, HashSet};

use luabox_diag::Severity;

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

    /// Set a tier-level override. Returns `false` (and does nothing) if the
    /// tier name or level keyword is unrecognised.
    pub fn set_tier(&mut self, tier: &str, level: &str) -> bool {
        match (Tier::parse(tier), Level::parse(level)) {
            (Some(tier), Some(level)) => {
                self.tiers.insert(tier, level);
                true
            }
            _ => false,
        }
    }

    /// Set a rule-id override. Returns `false` if the level keyword is
    /// unrecognised (the rule id itself is not validated here).
    pub fn set_rule(&mut self, rule_id: &str, level: &str) -> bool {
        match Level::parse(level) {
            Some(level) => {
                self.rules.insert(rule_id.to_owned(), level);
                true
            }
            None => false,
        }
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
