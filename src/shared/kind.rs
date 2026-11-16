//! What a task works from: a GitHub issue, a pull request, a spec file, or a pull request's
//! review to fix. The template side names it too, in the command that removes a
//! task still using a template.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Issue,
    Pr,
    /// A task whose source is a file instead of a GitHub issue (decision 174(d), issue 142): the
    /// id is given explicitly (`spec-<name>-<hash>`, see `task::record::spec_id`), never derived
    /// from `number`, which is unused (always 0).
    Spec,
    /// A task that fixes the must-fix findings of a pull request's latest sbxm review on that
    /// pull request's own branch (issue 313): `number` is the PR's, and the id names the head
    /// the review covered (`task::record::fix_id`), so a fix of a later head is a task of its
    /// own.
    Fix,
}

impl Kind {
    /// The start of a task id of this kind (`issue-5`), and the first key of the order tasks are
    /// listed in.
    pub(crate) fn prefix(self) -> &'static str {
        match self {
            Self::Issue => "issue",
            Self::Pr => "pr",
            Self::Spec => "spec",
            Self::Fix => "fix",
        }
    }
}
