//! The policy layer: what fairway does about the facts the checks
//! measure. The mechanics live in code; the policy lives in the
//! user's files under `policy/`, one file per domain, materialized
//! from the shipped defaults on first use and owned by the user
//! from then on.

#[cfg(test)]
mod tests;

use std::path::Path;

use serde::Deserialize;

use crate::config::ConfigError;
use crate::verdict::Verdict;

/// The git policy shipped with the binary. Kept as data, not code:
/// this file is the source of truth for every default.
pub const DEFAULT: &str = include_str!("../assets/policy/git.toml");

/// The policy for the git checks, one rule per check. The schema is
/// strict on purpose: every rule is required and unknown keys are
/// refused, so a typo or a file left behind by another version
/// fails loudly instead of silently meaning something else.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitPolicy {
    /// The rule for the `git_available` check.
    pub git_available: Rule,
    /// The rule for the `in_work_tree` check.
    pub in_work_tree: Rule,
    /// The rule for the `head_exists` check.
    pub head_exists: Rule,
    /// The rule for the `head_detached` check.
    pub head_detached: Rule,
    /// The rule for the `index_has_staged` check.
    pub index_has_staged: Rule,
    /// The rule for the `index_conflicted` check.
    pub index_conflicted: Rule,
    /// The rule for the `work_tree_clean` check.
    pub work_tree_clean: Rule,
    /// The rule for the `merge_in_progress` check.
    pub merge_in_progress: Rule,
    /// The rule for the `cherry_pick_in_progress` check.
    pub cherry_pick_in_progress: Rule,
    /// The rule for the `revert_in_progress` check.
    pub revert_in_progress: Rule,
    /// The rule for the `bisect_in_progress` check.
    pub bisect_in_progress: Rule,
    /// The rule for the `am_in_progress` check.
    pub am_in_progress: Rule,
    /// The rule for the `rebase_in_progress` check.
    pub rebase_in_progress: Rule,
}

impl GitPolicy {
    /// Parse and validate a policy file's text; `path` names the
    /// file in the error.
    ///
    /// # Errors
    ///
    /// The text is not TOML or does not match the schema.
    pub fn parse(text: &str, path: &Path) -> Result<GitPolicy, ConfigError> {
        toml::from_str(text).map_err(|source| ConfigError::new(path, source))
    }
}

/// One policy rule: when the check's answer equals `value`, the
/// rule fires — `prompt` goes to the agent and `action` decides
/// the verdict.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// The answer that fires the rule.
    pub value: bool,
    /// The instruction printed for the agent when the rule fires.
    pub prompt: String,
    /// The verdict rendered when the rule fires.
    pub action: Action,
}

/// What a fired rule does, named by the verdict it renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// Note the prompt and carry on (exit 0).
    Proceed,
    /// The agent must change course and retry (exit 1).
    Adjust,
    /// The agent must halt and report to the user (exit 2).
    Stop,
}

impl Action {
    /// The verdict this action renders, carrying `prompt` as the
    /// instruction.
    #[must_use]
    pub fn verdict(self, prompt: String) -> Verdict {
        match self {
            Action::Proceed => Verdict::Proceed(prompt),
            Action::Adjust => Verdict::Adjust(prompt),
            Action::Stop => Verdict::Stop(prompt),
        }
    }
}
