//! Rewriting an agent's commit messages before they reach the delivered branch (issue 244): a
//! worker's commit message can quote a closing keyword together with another issue's reference,
//! and GitHub reads that from a commit pushed to the default branch, not only from a PR
//! description (which [`super::review::defang`] already protects, issue 59).
//!
//! The agent's own, unrewritten commits live on [`super::repo::agent_ref`], never on the
//! delivered branch (`refs/heads/<branch>`) itself. [`rebuild`] rewrites every commit from the
//! task's starting point (exclusive) to that ref's tip (inclusive) onto the delivered branch,
//! keeping everything about each commit -- tree, parents, author, committer, both dates -- except
//! the message, which is defanged; a commit whose message needs no change still gets rebuilt on
//! its rebuilt parent, so its id only changes if an ancestor's did. Because the rebuild is a pure
//! function of the original commits, rebuilding the same range always gives the same ids: a rerun
//! or `task resume` never moves the delivered branch to something new. Every new commit is built
//! before the delivered branch moves, and it moves only if it is still where it was read to be, so
//! a failure partway through, or something else having moved it meanwhile, leaves it exactly where
//! it was.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};

use super::repo::{agent_ref, check_ref};
use super::review::defang;
use crate::git;

/// A commit's full identity, read once from its original (agent-made) object: everything
/// [`build_commit`] needs to recreate it with a different message. A signature header
/// (`gpgsig`) is read past, never kept: the rebuilt commit is unsigned.
struct Original {
    tree: String,
    parents: Vec<String>,
    author_name: String,
    author_email: String,
    author_date: String,
    committer_name: String,
    committer_email: String,
    committer_date: String,
    message: String,
}

/// Rewrites the delivered branch `branch` in `repo_git` from its agent ref (issue 244): every
/// commit from `start` (exclusive, a revision `repo_git` can resolve -- the task's base branch,
/// or for a task continuing a PR the PR head it fetched) up to the agent ref's tip (inclusive) is
/// rebuilt with a defanged message, and the delivered branch is moved to the result in one step.
///
/// `previous_agent_tip` is the agent ref's own tip as it stood just before the fetch that brought
/// in whatever is now beyond it (`None` when the agent ref didn't exist yet: a task's first
/// rebuild, or an old task backfilling it from the delivered branch). Since a rebuild is
/// deterministic, the delivered branch's tip right after the *previous* rebuild is exactly this
/// rebuild's own result for `previous_agent_tip`; if the branch isn't at that value now, something
/// other than the last rebuild moved it since, and this call refuses rather than adopting whatever
/// it currently finds as its new baseline. With `previous_agent_tip` as `None` there is no earlier
/// rebuild to compare against, so the branch's current value is trusted, as before.
pub fn rebuild(
    repo_git: &Path,
    start: &str,
    branch: &str,
    previous_agent_tip: Option<&str>,
) -> Result<()> {
    check_ref("branch", branch)?;
    let start_id = resolve(repo_git, start).with_context(|| {
        format!("cannot resolve {start} in the task repo; the task record may be damaged")
    })?;
    let agent = agent_ref(branch);
    let agent_tip = resolve(repo_git, &agent).with_context(|| {
        format!("{agent} is missing; collect the agent's commits before rewriting them")
    })?;
    let old_tip = resolve(repo_git, &format!("refs/heads/{branch}")).with_context(|| {
        format!("{branch} isn't in the task repo; the task record may be damaged")
    })?;

    let ids = commit_range(repo_git, &start_id, &agent_tip)?;
    let scratch = repo_git
        .parent()
        .unwrap_or(Path::new("."))
        .join("rewrite-message");
    let mut rebuilt: HashMap<String, String> = HashMap::new();
    let mut new_tip = start_id.clone();
    for id in &ids {
        let original = read_commit(repo_git, id)?;
        let parents: Vec<String> = original
            .parents
            .iter()
            .map(|p| rebuilt.get(p).cloned().unwrap_or_else(|| p.clone()))
            .collect();
        let message = defang(&original.message);
        new_tip = build_commit(repo_git, &scratch, &original, &parents, &message)?;
        rebuilt.insert(id.clone(), new_tip.clone());
    }

    let expected_old_tip = match previous_agent_tip {
        None => old_tip.clone(),
        Some(prev) if prev == start_id => start_id.clone(),
        Some(prev) => rebuilt.get(prev).cloned().with_context(|| {
            format!(
                "{prev} (the agent ref's tip before this rebuild) isn't an ancestor of {agent}'s \
                 current tip; the task record may be damaged"
            )
        })?,
    };
    if expected_old_tip != old_tip {
        bail!(
            "{branch} in the task repo was moved by something other than sbxm's own last rebuild; \
             nothing was changed"
        );
    }

    move_branch(repo_git, branch, &new_tip, &old_tip)
}

/// `rev` resolved to a full commit id in `repo_git`.
fn resolve(repo_git: &Path, rev: &str) -> Result<String> {
    let out = git::run(
        repo_git,
        Some(repo_git),
        None,
        &["rev-parse", "--verify", &format!("{rev}^{{commit}}")],
    )?;
    Ok(out.trim().to_owned())
}

/// The commits reachable from `tip` but not from `start`, oldest first and with every parent
/// before its children (so a mapped parent is always ready when a merge commit needs it).
fn commit_range(repo_git: &Path, start: &str, tip: &str) -> Result<Vec<String>> {
    let out = git::run(
        repo_git,
        Some(repo_git),
        None,
        &[
            "rev-list",
            "--reverse",
            "--topo-order",
            &format!("{start}..{tip}"),
        ],
    )?;
    Ok(out.lines().map(str::to_owned).collect())
}

/// Reads commit `id`'s tree, parents, author, committer and message straight from the object
/// (`git cat-file -p`), trusting nothing else about it: a header continuation line (the second and
/// later lines of a multi-line header such as `gpgsig` or `mergetag`) starts with a space and is
/// skipped, so a signature is read past rather than copied into the rebuilt commit.
fn read_commit(repo_git: &Path, id: &str) -> Result<Original> {
    let raw = git::run(repo_git, Some(repo_git), None, &["cat-file", "-p", id])
        .with_context(|| format!("cannot read commit {id} in the task repo"))?;
    let sep = raw
        .find("\n\n")
        .with_context(|| format!("commit {id} has no header/message separator"))?;
    let header = &raw[..sep];
    let message = raw[sep + 2..].to_owned();

    let mut tree = None;
    let mut parents = Vec::new();
    let mut author_line = None;
    let mut committer_line = None;
    for line in header.split('\n') {
        if line.starts_with(' ') {
            continue;
        }
        if let Some(v) = line.strip_prefix("tree ") {
            tree = Some(v.to_owned());
        } else if let Some(v) = line.strip_prefix("parent ") {
            parents.push(v.to_owned());
        } else if let Some(v) = line.strip_prefix("author ") {
            author_line = Some(v.to_owned());
        } else if let Some(v) = line.strip_prefix("committer ") {
            committer_line = Some(v.to_owned());
        }
    }
    let tree = tree.with_context(|| format!("commit {id} has no tree header"))?;
    let author_line = author_line.with_context(|| format!("commit {id} has no author header"))?;
    let committer_line =
        committer_line.with_context(|| format!("commit {id} has no committer header"))?;
    let (author_name, author_email, author_date) = split_identity(&author_line)
        .with_context(|| format!("commit {id}'s author header is unexpected: {author_line:?}"))?;
    let (committer_name, committer_email, committer_date) = split_identity(&committer_line)
        .with_context(|| {
            format!("commit {id}'s committer header is unexpected: {committer_line:?}")
        })?;
    Ok(Original {
        tree,
        parents,
        author_name,
        author_email,
        author_date,
        committer_name,
        committer_email,
        committer_date,
        message,
    })
}

/// Splits a `author`/`committer` header value (`Name <email> timestamp tz`) into its name, email
/// and `timestamp tz` (the exact form `GIT_AUTHOR_DATE`/`GIT_COMMITTER_DATE` accept).
fn split_identity(line: &str) -> Result<(String, String, String)> {
    let open = line.rfind('<').context("no `<` in the identity line")?;
    let close = line[open..]
        .find('>')
        .map(|i| i + open)
        .context("no `>` in the identity line")?;
    let name = line[..open].trim_end().to_owned();
    let email = line[open + 1..close].to_owned();
    let date = line[close + 1..].trim().to_owned();
    Ok((name, email, date))
}

/// Builds a new commit object with `original`'s tree and identity, `parents` (already rebuilt
/// where they needed to be) and `message`; returns its id. Nothing is written to any ref here.
fn build_commit(
    repo_git: &Path,
    scratch: &Path,
    original: &Original,
    parents: &[String],
    message: &str,
) -> Result<String> {
    std::fs::write(scratch, message.as_bytes())
        .with_context(|| format!("cannot write {}", scratch.display()))?;
    let scratch_text = scratch.to_str().with_context(|| {
        format!(
            "{} isn't valid UTF-8; use another folder",
            scratch.display()
        )
    })?;
    let mut args: Vec<&str> = vec!["commit-tree", &original.tree];
    for parent in parents {
        args.push("-p");
        args.push(parent);
    }
    args.push("-F");
    args.push(scratch_text);
    let identity = [
        ("GIT_AUTHOR_NAME", original.author_name.as_str()),
        ("GIT_AUTHOR_EMAIL", original.author_email.as_str()),
        ("GIT_AUTHOR_DATE", original.author_date.as_str()),
        ("GIT_COMMITTER_NAME", original.committer_name.as_str()),
        ("GIT_COMMITTER_EMAIL", original.committer_email.as_str()),
        ("GIT_COMMITTER_DATE", original.committer_date.as_str()),
    ];
    let out = git::run_with_identity(repo_git, Some(repo_git), None, &args, &identity)?;
    Ok(out.trim().to_owned())
}

/// Moves the delivered branch `branch` to `new_tip`, only if it is still at `old_tip`
/// (`git update-ref`'s own compare-and-swap): refuses, naming the branch and changing nothing,
/// if something else moved it since it was read.
fn move_branch(repo_git: &Path, branch: &str, new_tip: &str, old_tip: &str) -> Result<()> {
    let delivered = format!("refs/heads/{branch}");
    let out = git::output(
        repo_git,
        Some(repo_git),
        None,
        &["update-ref", &delivered, new_tip, old_tip],
    )?;
    if !out.status.success() {
        bail!(
            "{branch} in the task repo changed unexpectedly while its commits were being rewritten; nothing was changed"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_AUTHOR_DATE", "1700000000 +0000")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("GIT_COMMITTER_DATE", "1700000000 +0000")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    fn repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("repo.git");
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main", "--bare"]);
        (tmp, dir)
    }

    #[test]
    fn a_commit_with_nothing_to_defang_rebuilds_to_the_same_id() {
        let (_tmp, dir) = repo();
        let empty_tree = git(&dir, &["hash-object", "-t", "tree", "--stdin"]);
        let base = git(&dir, &["commit-tree", &empty_tree, "-m", "base"]);
        git(&dir, &["update-ref", "refs/heads/main", &base]);
        let agent = git(
            &dir,
            &[
                "commit-tree",
                &empty_tree,
                "-p",
                &base,
                "-m",
                "a plain message",
            ],
        );
        git(&dir, &["update-ref", "refs/sbxm/agent/issue-4", &agent]);
        git(&dir, &["update-ref", "refs/heads/issue-4", &base]);

        rebuild(&dir, "refs/heads/main", "issue-4", None).unwrap();

        assert_eq!(git(&dir, &["rev-parse", "refs/heads/issue-4"]), agent);
    }

    #[test]
    fn a_closing_reference_in_the_message_is_defanged() {
        let (_tmp, dir) = repo();
        let empty_tree = git(&dir, &["hash-object", "-t", "tree", "--stdin"]);
        let base = git(&dir, &["commit-tree", &empty_tree, "-m", "base"]);
        git(&dir, &["update-ref", "refs/heads/main", &base]);
        let agent = git(
            &dir,
            &[
                "commit-tree",
                &empty_tree,
                "-p",
                &base,
                "-m",
                "this also fixes #12",
            ],
        );
        git(&dir, &["update-ref", "refs/sbxm/agent/issue-4", &agent]);
        git(&dir, &["update-ref", "refs/heads/issue-4", &base]);

        rebuild(&dir, "refs/heads/main", "issue-4", None).unwrap();

        let rewritten = git(&dir, &["rev-parse", "refs/heads/issue-4"]);
        assert_ne!(rewritten, agent);
        let message = git(&dir, &["log", "-1", "--format=%B", &rewritten]);
        assert!(message.contains("fixes\u{200b} #12"), "{message}");
        // Tree, parent and identity are untouched.
        assert_eq!(
            git(&dir, &["rev-parse", &format!("{rewritten}^{{tree}}")]),
            empty_tree
        );
        assert_eq!(git(&dir, &["rev-parse", &format!("{rewritten}^1")]), base);
    }

    #[test]
    fn non_ascii_message_text_survives_except_for_the_inserted_zero_width_spaces() {
        let (_tmp, dir) = repo();
        let empty_tree = git(&dir, &["hash-object", "-t", "tree", "--stdin"]);
        let base = git(&dir, &["commit-tree", &empty_tree, "-m", "base"]);
        git(&dir, &["update-ref", "refs/heads/main", &base]);
        let agent = git(
            &dir,
            &[
                "commit-tree",
                &empty_tree,
                "-p",
                &base,
                "-m",
                "héllo @bob 日本語 that also closes #7",
            ],
        );
        git(&dir, &["update-ref", "refs/sbxm/agent/issue-4", &agent]);
        git(&dir, &["update-ref", "refs/heads/issue-4", &base]);

        rebuild(&dir, "refs/heads/main", "issue-4", None).unwrap();

        let rewritten = git(&dir, &["rev-parse", "refs/heads/issue-4"]);
        let message = git(&dir, &["log", "-1", "--format=%B", &rewritten]);
        assert_eq!(
            message,
            "héllo @\u{200b}bob 日本語 that also closes\u{200b} #7"
        );
    }

    #[test]
    fn a_merge_commit_keeps_both_rebuilt_parents() {
        let (_tmp, dir) = repo();
        let empty_tree = git(&dir, &["hash-object", "-t", "tree", "--stdin"]);
        let base = git(&dir, &["commit-tree", &empty_tree, "-m", "base"]);
        git(&dir, &["update-ref", "refs/heads/main", &base]);
        let main_line = git(
            &dir,
            &[
                "commit-tree",
                &empty_tree,
                "-p",
                &base,
                "-m",
                "main line work",
            ],
        );
        let side = git(
            &dir,
            &[
                "commit-tree",
                &empty_tree,
                "-p",
                &base,
                "-m",
                "side work, closes #5",
            ],
        );
        let merge = git(
            &dir,
            &[
                "commit-tree",
                &empty_tree,
                "-p",
                &main_line,
                "-p",
                &side,
                "-m",
                "merge side, fixes #9",
            ],
        );
        git(&dir, &["update-ref", "refs/sbxm/agent/issue-4", &merge]);
        git(&dir, &["update-ref", "refs/heads/issue-4", &base]);

        rebuild(&dir, "refs/heads/main", "issue-4", None).unwrap();

        let rewritten = git(&dir, &["rev-parse", "refs/heads/issue-4"]);
        assert_ne!(rewritten, merge);
        let p1 = git(&dir, &["rev-parse", &format!("{rewritten}^1")]);
        let p2 = git(&dir, &["rev-parse", &format!("{rewritten}^2")]);
        // `main_line`'s message has nothing to defang, so it rebuilds to the same id; `side`'s
        // does, so it doesn't.
        assert_eq!(p1, main_line);
        assert_ne!(p2, side);
        let merge_message = git(&dir, &["log", "-1", "--format=%B", &rewritten]);
        assert!(
            merge_message.contains("fixes\u{200b} #9"),
            "{merge_message}"
        );
        let side_message = git(&dir, &["log", "-1", "--format=%B", &p2]);
        assert!(side_message.contains("closes\u{200b} #5"), "{side_message}");
    }

    /// Crafts a commit object by hand (`git hash-object -t commit -w --stdin`), so a fake
    /// signature can be attached without a real GPG key.
    fn write_commit_object(dir: &Path, content: &str) -> String {
        use std::io::Write;
        let mut child = Command::new("git")
            .current_dir(dir)
            .args(["hash-object", "-t", "commit", "-w", "--stdin"])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(content.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    #[test]
    fn a_signed_commit_is_rebuilt_unsigned() {
        let (_tmp, dir) = repo();
        let empty_tree = git(&dir, &["hash-object", "-t", "tree", "--stdin"]);
        let base = git(&dir, &["commit-tree", &empty_tree, "-m", "base"]);
        git(&dir, &["update-ref", "refs/heads/main", &base]);
        let raw = format!(
            "tree {empty_tree}\nparent {base}\nauthor t <t@t> 1700000000 +0000\ncommitter t <t@t> 1700000000 +0000\ngpgsig -----BEGIN PGP SIGNATURE-----\n fakefakefakefakefakefake\n fakefake=\n -----END PGP SIGNATURE-----\n\nsigned work\n"
        );
        let agent = write_commit_object(&dir, &raw);
        git(&dir, &["update-ref", "refs/sbxm/agent/issue-4", &agent]);
        git(&dir, &["update-ref", "refs/heads/issue-4", &base]);

        rebuild(&dir, "refs/heads/main", "issue-4", None).unwrap();

        let rewritten = git(&dir, &["rev-parse", "refs/heads/issue-4"]);
        // The gpgsig header makes the original object differ from any unsigned rebuild of the
        // same tree, parent, identity and message, so this alone proves it was dropped.
        assert_ne!(rewritten, agent);
        let full = git(&dir, &["cat-file", "-p", &rewritten]);
        assert!(!full.contains("gpgsig"), "{full}");
        let message = git(&dir, &["log", "-1", "--format=%B", &rewritten]);
        assert_eq!(message, "signed work");
    }

    /// A commit whose `tree` names an object the repo doesn't have: `git commit-tree` fails on it,
    /// the way it would on any commit partway through a longer range (the issue's own example).
    #[test]
    fn a_failure_partway_through_the_rebuild_leaves_the_delivered_branch_unchanged() {
        let (_tmp, dir) = repo();
        let empty_tree = git(&dir, &["hash-object", "-t", "tree", "--stdin"]);
        let base = git(&dir, &["commit-tree", &empty_tree, "-m", "base"]);
        git(&dir, &["update-ref", "refs/heads/main", &base]);
        git(&dir, &["update-ref", "refs/heads/issue-4", &base]);
        let c1 = git(
            &dir,
            &["commit-tree", &empty_tree, "-p", &base, "-m", "first"],
        );
        let bogus_tree = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
        let c2 = write_commit_object(
            &dir,
            &format!(
                "tree {bogus_tree}\nparent {c1}\nauthor t <t@t> 1700000000 +0000\ncommitter t <t@t> 1700000000 +0000\n\nsecond\n"
            ),
        );
        let c3 = git(
            &dir,
            &["commit-tree", &empty_tree, "-p", &c2, "-m", "third"],
        );
        git(&dir, &["update-ref", "refs/sbxm/agent/issue-4", &c3]);

        let err = format!(
            "{:#}",
            rebuild(&dir, "refs/heads/main", "issue-4", None).unwrap_err()
        );

        assert!(!err.is_empty());
        assert_eq!(git(&dir, &["rev-parse", "refs/heads/issue-4"]), base);
    }

    /// `task resume` (or any rerun) calls [`rebuild`] again over the same range: the result must
    /// be the same ids, not a fresh set.
    #[test]
    fn rebuilding_the_same_range_twice_gives_the_same_ids() {
        let (_tmp, dir) = repo();
        let empty_tree = git(&dir, &["hash-object", "-t", "tree", "--stdin"]);
        let base = git(&dir, &["commit-tree", &empty_tree, "-m", "base"]);
        git(&dir, &["update-ref", "refs/heads/main", &base]);
        git(&dir, &["update-ref", "refs/heads/issue-4", &base]);
        let agent = git(
            &dir,
            &[
                "commit-tree",
                &empty_tree,
                "-p",
                &base,
                "-m",
                "also fixes #4",
            ],
        );
        git(&dir, &["update-ref", "refs/sbxm/agent/issue-4", &agent]);

        rebuild(&dir, "refs/heads/main", "issue-4", None).unwrap();
        let first_tip = git(&dir, &["rev-parse", "refs/heads/issue-4"]);
        rebuild(&dir, "refs/heads/main", "issue-4", None).unwrap();
        let second_tip = git(&dir, &["rev-parse", "refs/heads/issue-4"]);

        assert_ne!(first_tip, agent);
        assert_eq!(first_tip, second_tip);
    }

    /// [`move_branch`]'s compare-and-swap is `rebuild`'s only way to move the delivered branch, so
    /// this is also the backstop for two rebuilds racing on the same task (decision: the record's
    /// own running check, tested elsewhere, is the first line of defence; this is what still holds
    /// if it is ever bypassed).
    #[test]
    fn move_branch_refuses_when_the_branch_is_not_at_the_expected_old_value() {
        let (_tmp, dir) = repo();
        let empty_tree = git(&dir, &["hash-object", "-t", "tree", "--stdin"]);
        let base = git(&dir, &["commit-tree", &empty_tree, "-m", "base"]);
        let other = git(&dir, &["commit-tree", &empty_tree, "-m", "elsewhere"]);
        let new = git(&dir, &["commit-tree", &empty_tree, "-m", "new"]);
        git(&dir, &["update-ref", "refs/heads/issue-4", &other]);

        let message = format!(
            "{:#}",
            move_branch(&dir, "issue-4", &new, &base).unwrap_err()
        );

        assert!(message.contains("issue-4"), "{message}");
        assert_eq!(git(&dir, &["rev-parse", "refs/heads/issue-4"]), other);
    }

    /// Issue 244's required case, distinct from the CAS test above: a branch moved **since the
    /// last rebuild**, not merely during this call. Passing the first rebuild's own
    /// `previous_agent_tip` (`None`, its first ever) let that one through as always; the second
    /// rebuild must recognise that the branch it now finds isn't what its own previous rebuild
    /// left, and refuse rather than overwriting it.
    #[test]
    fn a_branch_moved_since_the_last_rebuild_is_refused() {
        let (_tmp, dir) = repo();
        let empty_tree = git(&dir, &["hash-object", "-t", "tree", "--stdin"]);
        let base = git(&dir, &["commit-tree", &empty_tree, "-m", "base"]);
        git(&dir, &["update-ref", "refs/heads/main", &base]);
        git(&dir, &["update-ref", "refs/heads/issue-4", &base]);
        let first = git(
            &dir,
            &["commit-tree", &empty_tree, "-p", &base, "-m", "first"],
        );
        git(&dir, &["update-ref", "refs/sbxm/agent/issue-4", &first]);
        rebuild(&dir, "refs/heads/main", "issue-4", None).unwrap();

        // Something other than sbxm moves the delivered branch before the next rebuild.
        let elsewhere = git(&dir, &["commit-tree", &empty_tree, "-m", "elsewhere"]);
        git(&dir, &["update-ref", "refs/heads/issue-4", &elsewhere]);

        // A fix round's second bundle advances the agent ref further.
        let second = git(
            &dir,
            &["commit-tree", &empty_tree, "-p", &first, "-m", "second"],
        );
        git(&dir, &["update-ref", "refs/sbxm/agent/issue-4", &second]);

        let err = format!(
            "{:#}",
            rebuild(&dir, "refs/heads/main", "issue-4", Some(first.as_str())).unwrap_err()
        );

        assert!(err.contains("issue-4"), "{err}");
        assert_eq!(git(&dir, &["rev-parse", "refs/heads/issue-4"]), elsewhere);
        assert_eq!(git(&dir, &["rev-parse", "refs/sbxm/agent/issue-4"]), second);
    }
}
