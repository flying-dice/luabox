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
//!
//! The whole translation lives here now, as [`LintConfig::from_manifest`],
//! which is also where a `[lint]` key that names no known rule is caught: the
//! manifest parser cannot check rule ids (they are this crate's), so the check
//! happens where the config is consumed and comes back as
//! [`UnknownRuleId`]s the frontends surface.

use std::collections::{HashMap, HashSet};

use luabox_diag::Severity;
use luabox_manifest::model::Lint;
pub use luabox_manifest::model::{LintLevel, LintTier};

use crate::rule::{Rule, Tier};
use crate::rules::rule_ids;
use crate::suggest;

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

/// A `[lint]` key that named neither a known rule id nor a tier.
///
/// The manifest parser cannot catch this: rule ids live here, and
/// `luabox-manifest` must stay dependency-free, so every key that is not
/// `globals` and not a tier name is recorded as a rule-id override
/// unvalidated. The check therefore belongs where the config is *consumed* —
/// [`LintConfig::unknown_rule_ids`] — and this is what it reports (CC-M8:
/// `unused-locl = "allow"` used to do nothing, silently).
///
/// The suggestion spans rule ids *and* tier names, because a mistyped tier
/// (`pedantics = "warn"`) is indistinguishable from a rule-id override by the
/// time it gets here and should be nudged back at the tier it meant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownRuleId {
    id: String,
    suggestion: Option<&'static str>,
}

impl UnknownRuleId {
    /// The `[lint]` key as written.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The closest known rule id or tier name, if one is within a typo's
    /// reach.
    #[must_use]
    pub fn suggestion(&self) -> Option<&'static str> {
        self.suggestion
    }

    /// The one-line headline, shared by every consumer so the CLI and the
    /// editor word this the same way.
    #[must_use]
    pub fn message(&self) -> String {
        format!("unknown lint rule id `{}` in `[lint]`", self.id)
    }

    /// The trailing notes: the "did you mean" nudge when there is one, then
    /// the consequence — the entry is inert, which is the whole reason this
    /// is reported at all.
    #[must_use]
    pub fn notes(&self) -> Vec<String> {
        let mut notes = Vec::new();
        if let Some(candidate) = self.suggestion {
            notes.push(format!("did you mean `{candidate}`?"));
        }
        notes.push("this `[lint]` entry has no effect".to_owned());
        notes
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

    /// Build the effective configuration from a manifest `[lint]` table,
    /// alongside every key that named no known rule id (see
    /// [`UnknownRuleId`]).
    ///
    /// This is the *only* `[lint]` → [`LintConfig`] translation in the
    /// workspace: `luabox-cli` and `luabox-lsp` each used to keep their own
    /// copy (CC-M8), which is precisely how a validation step gets added to
    /// one frontend and not the other. Returning a pair rather than swallowing
    /// the unknown ids is deliberate — a caller has to name the second half to
    /// discard it.
    #[must_use]
    pub fn from_manifest(lint: &Lint) -> (Self, Vec<UnknownRuleId>) {
        let mut config = Self::new();
        for name in &lint.globals {
            config.allow_global(name.clone());
        }
        for (&tier, &level) in &lint.tiers {
            config.set_tier(tier.into(), level.into());
        }
        for (rule, &level) in &lint.rules {
            config.set_rule(rule, level.into());
        }
        let unknown = config.unknown_rule_ids();
        (config, unknown)
    }

    /// Every rule-id override that names no rule this build has, each with a
    /// "did you mean" candidate drawn from the known rule ids *and* the tier
    /// names. Sorted by id, so a report over them is deterministic.
    ///
    /// Empty for a configuration that only names rules that exist — the
    /// silent case, which stays silent.
    #[must_use]
    pub fn unknown_rule_ids(&self) -> Vec<UnknownRuleId> {
        let known = rule_ids();
        let candidates: Vec<&'static str> = known
            .iter()
            .copied()
            .chain(Tier::ALL.iter().map(|tier| tier.name()))
            .collect();
        let mut unknown: Vec<UnknownRuleId> = self
            .rules
            .keys()
            .filter(|id| !known.contains(&id.as_str()))
            .map(|id| UnknownRuleId {
                id: id.clone(),
                suggestion: suggest::closest(id, candidates.iter().copied()),
            })
            .collect();
        unknown.sort_by(|a, b| a.id.cmp(&b.id));
        unknown
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
    ///
    /// An id no rule answers to is stored and stays inert; ask
    /// [`Self::unknown_rule_ids`] for those, or build through
    /// [`Self::from_manifest`], which hands them back with the config.
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
