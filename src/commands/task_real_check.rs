//! `sbxm task real-check --pr N (--pass | --fail) "<note>"` (issue 547, #460 guardrail 3): records
//! the result of an issue's real check (decision 273, [`real_check`]) on its PR, as the commit
//! status [`real_check::CONTEXT`] (`success` or `failure`) on the PR's current head commit, the
//! note as its description. `task loop` arms the PR only while its head has `success`, so a later
//! push needs the check run and posted again. The note and the PR are checked before anything is
//! posted.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use super::task_start::resolve_repo;
use crate::github::{CommitStatus, GitHubBackend, PrState, StatusState};
use crate::task::real_check::{self, NOTE_LIMIT};

pub struct Options {
    /// The checkout whose origin names the GitHub repo.
    pub repo_root: PathBuf,
    /// For tests: the GitHub repo, `owner/name`, instead of the checkout's origin.
    pub repo: Option<String>,
    pub pr: u32,
    /// `--pass`; `false` for `--fail`.
    pub pass: bool,
    pub note: String,
}

/// Posts the real check's result on PR `opts.pr`'s head and says on which commit.
pub fn run(opts: &Options, github: &dyn GitHubBackend, out: &mut dyn Write) -> Result<()> {
    let note = opts.note.trim();
    if note.is_empty() {
        bail!("the note is empty: say what was run and what came out of it");
    }
    let chars = note.chars().count();
    if chars > NOTE_LIMIT {
        bail!(
            "the note has {chars} characters, over GitHub's limit of {NOTE_LIMIT} for a status's \
             description; shorten it"
        );
    }
    let repo = resolve_repo(opts.repo.as_deref(), &opts.repo_root)?;
    let number = opts.pr;
    let pr = github
        .pr(&repo, number)
        .with_context(|| format!("cannot read PR #{number}; nothing was posted"))?;
    match pr.state {
        PrState::Open => {}
        PrState::Closed => bail!("PR #{number} is closed; nothing was posted"),
        PrState::Merged => bail!("PR #{number} is merged; nothing was posted"),
    }
    let head = pr.head_oid;
    if !matches!(head.len(), 40 | 64) || !head.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!(
            "PR #{number}'s head `{head}` is not a full commit id, so no status can be posted on \
             it"
        );
    }
    let state = if opts.pass {
        StatusState::Success
    } else {
        StatusState::Failure
    };
    let status = CommitStatus {
        state,
        context: real_check::CONTEXT.to_owned(),
        description: note.to_owned(),
    };
    github
        .commit_status(&repo, &head, &status)
        .with_context(|| format!("cannot post {} on PR #{number}", real_check::CONTEXT))?;
    writeln!(
        out,
        "PR #{number}: posted {} = {} on its head {head}; a later push needs the check run and \
         posted again",
        real_check::CONTEXT,
        state.word()
    )?;
    Ok(())
}
