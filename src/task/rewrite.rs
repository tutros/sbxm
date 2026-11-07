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
//! it was. [`rebuilt_marker`] records which agent tip that last successful move rebuilt, moved
//! atomically with the branch itself, so a fetch that advances the agent ref without a rebuild
//! ever following it through (the rebuild crashed, or failed for a reason since fixed) is never
//! mistaken for a rebuild that did.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result, bail};

use super::repo::{agent_ref, check_ref, no_hooks_config, resolve_commits};
use super::review::defang;
use crate::git;

/// A commit's full identity, read once from its original (agent-made) object: everything
/// [`build_commit`] needs to recreate it with a different message. `header_lines` is every
/// header line verbatim (review round 2, M-3: not only the specific fields `rebuild` happens to
/// use by name, so a header such as `encoding` survives byte for byte too), except a signature
/// header (`gpgsig`) and its continuation lines, which are dropped: the rebuilt commit is
/// unsigned. A `parent` line is kept in `header_lines` at its original position so
/// [`build_commit`] can substitute the rebuilt id there without disturbing anything else's
/// order. `message` is kept as raw bytes, not a `String`: an `encoding` header other than UTF-8
/// means it need not be valid UTF-8 at all (issue 244's encoding edge case), and
/// [`defang_bytes`] preserves it exactly either way.
struct Original {
    tree: String,
    parents: Vec<String>,
    header_lines: Vec<Vec<u8>>,
    message: Vec<u8>,
    /// The object exactly as git stores it: a rebuild that comes out byte for byte the same is
    /// already in the repo, so nothing needs writing (issue #444).
    raw: Vec<u8>,
}

/// The largest commit object (header plus message) [`rebuild`] reads (PR 250 review, M-1;
/// decision 198): a highly compressible message can be tiny in the bundle and huge once read, and
/// rebuilding holds several copies of it, so a bigger commit is refused before it is read. A real
/// commit is never close to this.
pub const MAX_COMMIT_BYTES: u64 = 1024 * 1024;

/// The most commit-object bytes [`rebuild`] reads in one `cat-file --batch` (issue #444).
const BATCH_BYTES: u64 = 4 * MAX_COMMIT_BYTES;

/// What [`check_range`] learned about one commit before its body is read.
struct Checked {
    tree: String,
    size: u64,
}

/// The ref recording the agent tip whose rebuild is on the delivered branch right now (issue 244
/// review round 2, M-1). It moves only together with the delivered branch, in the same atomic
/// `update-ref --stdin` transaction [`move_branch`] runs, so it can never point past a rebuild
/// that didn't actually land: a fetch that advances the agent ref doesn't move it, only a
/// *successful* [`rebuild`] does. Reading it tells this call apart from a delivered branch moved
/// by something other than sbxm's own last rebuild, even when a fetch happened in between without
/// a rebuild ever following it through (a crash, or a failure since fixed).
fn rebuilt_marker(branch: &str) -> String {
    format!("refs/sbxm/rebuilt/{branch}")
}

/// Rewrites the delivered branch `branch` in `repo_git` from its agent ref (issue 244): every
/// commit from `start` (exclusive, a revision `repo_git` can resolve -- the task's base branch,
/// or for a task continuing a PR the PR head it fetched) up to the agent ref's tip (inclusive) is
/// rebuilt with a defanged message, and the delivered branch is moved to the result in one step.
///
/// Since a rebuild is deterministic, the delivered branch's tip right after the *previous*
/// rebuild is exactly what rebuilding [`rebuilt_marker`]'s own recorded agent tip would produce
/// again now; if the branch isn't at that value, something other than the last rebuild moved it
/// since, and this call refuses rather than adopting whatever it currently finds as its new
/// baseline. A missing marker (no earlier successful rebuild: a task's first one, or an old task
/// backfilling the agent ref from the delivered branch) trusts the branch's current value, as
/// before.
pub fn rebuild(repo_git: &Path, start: &str, branch: &str) -> Result<()> {
    check_ref("branch", branch)?;
    let agent = agent_ref(branch);
    let marker = rebuilt_marker(branch);
    let branch_ref = format!("refs/heads/{branch}");
    // One git start answers all four lookups; each missing one is reported by its own message.
    let found = resolve_commits(repo_git, &[start, &agent, &branch_ref, &marker])?;
    let start_id = start_commit(start, &found[0])?;
    let agent_tip = found[1].clone().with_context(|| {
        format!("{agent} is missing; collect the agent's commits before rewriting them")
    })?;
    rebuild_from(
        repo_git,
        branch,
        Read {
            start_id,
            agent_tip,
            old_tip: found[2].clone(),
            last_rebuilt: found[3].clone(),
        },
    )?;
    Ok(())
}

/// Fetches the agent's bundle from `workspace` onto its agent ref
/// ([`super::repo::fetch_bundle`]) and [`rebuild`]s the delivered branch from it, with one lookup
/// for both (issue #444): the refs the rebuild needs are read before the fetch, which moves only
/// the agent ref, and the agent ref's new tip is the one the bundle names. A `start` that doesn't
/// resolve is refused before the fetch. Returns how many commits the delivered branch now has
/// past `start`, from the rebuild's own range where that tells, else counted.
pub fn fetch_and_rebuild(
    repo_git: &Path,
    workspace: &Path,
    start: &str,
    branch: &str,
    cap: u64,
) -> Result<u32> {
    check_ref("branch", branch)?;
    let marker = rebuilt_marker(branch);
    let fetched = super::repo::fetch_bundle_resolving(
        repo_git,
        workspace,
        branch,
        cap,
        &[start, &marker],
        |found| start_commit(start, &found[0]).map(|_| ()),
    )?;
    let start_id = start_commit(start, &fetched.extra[0])?;
    let ahead = rebuild_from(
        repo_git,
        branch,
        Read {
            start_id: start_id.clone(),
            agent_tip: fetched.agent_tip,
            old_tip: fetched.branch_tip,
            last_rebuilt: fetched.extra[1].clone(),
        },
    )?;
    match ahead {
        Some(ahead) => Ok(ahead),
        None => super::repo::commits_since(repo_git, &start_id, branch),
    }
}

/// `start` resolved, or the error naming it.
fn start_commit(start: &str, found: &Option<String>) -> Result<String> {
    found.clone().with_context(|| {
        format!("cannot resolve {start} in the task repo; the task record may be damaged")
    })
}

/// What [`rebuild_from`] starts from, read by its caller in one lookup.
struct Read {
    start_id: String,
    /// The agent ref's tip; [`move_branch`] moves nothing unless the agent ref is still there.
    agent_tip: String,
    old_tip: Option<String>,
    last_rebuilt: Option<String>,
}

/// [`rebuild`] from refs its caller already read; returns [`ahead_from_range`].
fn rebuild_from(repo_git: &Path, branch: &str, read: Read) -> Result<Option<u32>> {
    let Read {
        start_id,
        agent_tip,
        old_tip,
        last_rebuilt,
    } = read;
    let agent = agent_ref(branch);
    let marker = rebuilt_marker(branch);
    let old_tip = old_tip.with_context(|| {
        format!("{branch} isn't in the task repo; the task record may be damaged")
    })?;

    let listed = commit_range(repo_git, &start_id, &agent_tip, branch)?;
    let range = &listed.commits;
    let checked = check_range(repo_git, &agent_tip, range)?;
    let mut rebuilt: HashMap<String, String> = HashMap::new();
    // Bodies are read a batch at a time, each batch within `BATCH_BYTES` (a commit over
    // `MAX_COMMIT_BYTES` was refused by `check_range`, so one always fits), and dropped before the
    // next: the memory held stays bounded however long the history is (decision 198).
    let mut from = 0;
    while from < range.len() {
        let mut to = from;
        let mut bytes = 0;
        while to < range.len() && (to == from || bytes + checked[to].size <= BATCH_BYTES) {
            bytes += checked[to].size;
            to += 1;
        }
        let ids: Vec<String> = range[from..to].iter().map(|(id, _)| id.clone()).collect();
        let originals = read_commits(repo_git, &ids)?;
        for ((id, parents), (checked, original)) in range[from..to]
            .iter()
            .zip(checked[from..to].iter().zip(&originals))
        {
            if original.tree != checked.tree || &original.parents != parents {
                bail!(
                    "commit {id} doesn't read the same through git's two views of it; the commit being rebuilt is damaged"
                );
            }
            let parents: Vec<String> = original
                .parents
                .iter()
                .map(|p| rebuilt.get(p).cloned().unwrap_or_else(|| p.clone()))
                .collect();
            let message = defang_bytes(&original.message);
            let new_id = build_commit(repo_git, id, original, &parents, &message)?;
            rebuilt.insert(id.clone(), new_id);
        }
        from = to;
    }
    // A commit outside the rebuilt range (the start itself, or one `commit_range` left out as
    // already on another branch -- the agent tip too, when the worker only fast-forwarded to an
    // existing branch, review round 4, M-1) is delivered as it is.
    let delivered = |id: &str| rebuilt.get(id).cloned().unwrap_or_else(|| id.to_owned());
    let new_tip = delivered(&agent_tip);

    let expected_old_tip = match last_rebuilt.as_deref() {
        None => old_tip.clone(),
        Some(prev) => {
            if !rebuilt.contains_key(prev) && !is_ancestor(repo_git, prev, &agent_tip)? {
                bail!(
                    "{prev} (the agent tip of sbxm's last rebuild) isn't an ancestor of {agent}'s \
                     current tip; the task record may be damaged"
                );
            }
            delivered(prev)
        }
    };
    if expected_old_tip != old_tip {
        bail!(
            "{branch} in the task repo was moved by something other than sbxm's own last rebuild; \
             nothing was changed"
        );
    }

    move_branch(
        repo_git,
        branch,
        &new_tip,
        &old_tip,
        &marker,
        last_rebuilt.as_deref(),
        &agent_tip,
    )?;
    Ok(ahead_from_range(&listed, &start_id, &rebuilt))
}

/// Whether commit `ancestor` is `tip` or one of its ancestors in `repo_git`.
fn is_ancestor(repo_git: &Path, ancestor: &str, tip: &str) -> Result<bool> {
    let out = git::output(
        repo_git,
        Some(repo_git),
        None,
        &["merge-base", "--is-ancestor", ancestor, tip],
    )?;
    Ok(out.status.success())
}

/// The commits reachable from `tip` but not from `start` and not already reachable from any
/// *other* branch or tag in `repo_git` (review round 2, M-2): a commit an agent-made merge brings
/// in from a branch that already existed -- already on GitHub, or another task's own -- is not
/// the agent's to rewrite even though it is reachable from `tip`, so it is excluded here and
/// kept as an out-of-range parent, exactly as it was, by [`rebuild`]'s own fallback for a parent
/// that `rebuilt` never got an entry for. `branch` (the delivered branch being rebuilt) is
/// excluded from that exclusion (`--exclude`): its own current tip is this same rebuild's
/// previous result, which can be identical to one of today's original ids when nothing needed
/// defanging, and that id is still the agent's own commit to rebuild again, not an outside one
/// to protect. Oldest first and with every parent before its children (so a mapped parent is
/// always ready when a merge commit needs it).
fn commit_range(repo_git: &Path, start: &str, tip: &str, branch: &str) -> Result<Range> {
    let out = git::run(
        repo_git,
        Some(repo_git),
        None,
        &[
            "rev-list",
            "--reverse",
            "--topo-order",
            "--parents",
            "--boundary",
            tip,
            "--exclude",
            branch,
            "--not",
            start,
            "--branches",
            "--tags",
        ],
    )?;
    // `<id> <parent> <parent>...` per line (issue #444: the parents come with the range, so the
    // existence check needs no second look at each commit), and `-<id>` for a boundary commit.
    let mut range = Range::default();
    for line in out.lines() {
        let mut ids = line.split_whitespace().map(str::to_owned);
        let id = ids.next().unwrap_or_default();
        match id.strip_prefix('-') {
            Some(boundary) => range.boundary.push(boundary.to_owned()),
            None => range.commits.push((id, ids.collect())),
        }
    }
    Ok(range)
}

/// What [`commit_range`] lists: the commits to rebuild, and the boundary (`--boundary`): every
/// commit outside the range that is a parent of one inside it.
#[derive(Default)]
struct Range {
    commits: Vec<(String, Vec<String>)>,
    boundary: Vec<String>,
}

/// How many commits the rebuilt branch has past `start_id`, when the range alone tells (issue
/// #444): every commit outside the range that one inside it builds on is `start_id` itself, and
/// every commit inside it has a parent (so each descends from `start_id`, and so does its rebuilt
/// copy, which can't then be reachable from `start_id`), and no two commits were rebuilt into the
/// same one. The rebuilt branch then holds exactly the rebuilt range on top of `start_id`. `None`
/// otherwise (an agent-made merge of another branch, a root commit, an empty range): count it.
fn ahead_from_range(
    range: &Range,
    start_id: &str,
    rebuilt: &HashMap<String, String>,
) -> Option<u32> {
    let only_start = !range.boundary.is_empty() && range.boundary.iter().all(|b| b == start_id);
    let no_roots = range.commits.iter().all(|(_, parents)| !parents.is_empty());
    let distinct = rebuilt.values().collect::<HashSet<_>>().len() == rebuilt.len();
    (only_start && no_roots && distinct && rebuilt.len() == range.commits.len())
        .then(|| u32::try_from(range.commits.len()).ok())
        .flatten()
}

/// One git start for everything `rebuild` must know before it reads a body or writes anything
/// (the agent's `tip` itself is a commit, too):
/// every commit of `range` is a commit within [`MAX_COMMIT_BYTES`] (refused before it is read,
/// PR 250 review, M-1), its tree exists, and so does every parent outside the range (a parent
/// inside it is rebuilt, so its new id exists by the time it is used). `commit-tree` would check
/// the tree and parents before it wrote anything; this keeps that guarantee for
/// [`build_commit`]'s `hash-object`: a commit whose tree or a parent is missing -- the way a
/// partial, corrupted range would be -- fails before anything is written. Returns each commit's
/// tree id (for [`rebuild`] to compare with the one it reads from the commit itself) and size.
fn check_range(
    repo_git: &Path,
    tip: &str,
    range: &[(String, Vec<String>)],
) -> Result<Vec<Checked>> {
    let in_range: HashSet<&str> = range.iter().map(|(id, _)| id.as_str()).collect();
    let outside: BTreeSet<&str> = range
        .iter()
        .flat_map(|(_, parents)| parents)
        .map(String::as_str)
        .filter(|parent| !in_range.contains(parent))
        .collect();
    let mut names: Vec<String> = range.iter().map(|(id, _)| id.clone()).collect();
    names.extend(range.iter().map(|(id, _)| format!("{id}^{{tree}}")));
    names.extend(outside.iter().map(|parent| format!("{parent}^{{commit}}")));
    names.push(tip.to_owned());
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut found = git::check_objects(repo_git, Some(repo_git), None, &names)
        .context("cannot read the commits in the task repo")?;

    // The agent's tip as the bundle named it, not peeled (issue #444): it becomes the delivered
    // branch's tip as it is when nothing in the range needs rebuilding, so it must be a commit.
    if found
        .pop()
        .flatten()
        .is_none_or(|object| object.kind != "commit")
    {
        bail!("the agent's tip {tip} isn't a commit in the task repo");
    }
    let (commits, rest) = found.split_at(range.len());
    let (trees, parents) = rest.split_at(range.len());
    for ((id, _), object) in range.iter().zip(commits) {
        let object = object
            .as_ref()
            .with_context(|| format!("cannot read the size of commit {id} in the task repo"))?;
        if object.kind != "commit" {
            bail!("{id} is a {}, not a commit, in the task repo", object.kind);
        }
        if object.size > MAX_COMMIT_BYTES {
            bail!(
                "commit {id} is {} bytes, over the {MAX_COMMIT_BYTES}-byte limit for one commit; \
                 ask the agent for a shorter commit message",
                object.size
            );
        }
    }
    for (parent, object) in outside.iter().zip(parents) {
        if object.is_none() {
            bail!(
                "commit {parent} doesn't exist in the task repo; the commit being rebuilt is damaged"
            );
        }
    }
    range
        .iter()
        .zip(commits.iter().zip(trees))
        .map(|((id, _), (commit, tree))| match (commit, tree) {
            (Some(commit), Some(tree)) if tree.kind == "tree" => Ok(Checked {
                tree: tree.id.clone(),
                size: commit.size,
            }),
            _ => bail!(
                "the tree of commit {id} doesn't exist in the task repo; the commit being rebuilt is damaged"
            ),
        })
        .collect()
}

/// Reads commit `id`'s tree, parents, every header line and the message straight from the object
/// (`git cat-file -p`), trusting nothing else about it: a header continuation line (the second
/// and later lines of a multi-line header such as `gpgsig` or `mergetag`) starts with a space and
/// belongs to whichever header line started it, so a signature (and only a signature) is read
/// past rather than copied into the rebuilt commit, continuation lines included. Every other
/// header line -- `tree`, `parent`, `author`, `committer`, `encoding`, anything else git or a
/// hook ever writes there -- is kept verbatim in `header_lines` (review round 2, M-3), in its
/// original order, so [`build_commit`] only ever substitutes a `parent` line's id and otherwise
/// reproduces the header exactly. Read as raw bytes (not decoded as UTF-8) so the message -- the
/// one part of a commit object that need not be UTF-8 at all (an `encoding` header can say
/// otherwise) -- survives untouched, and so does every header line (review round 4, M-2: an
/// author name need not be UTF-8 either); only the tree and parent ids, which git always writes
/// as ASCII hex, are decoded.
fn read_commits(repo_git: &Path, ids: &[String]) -> Result<Vec<Original>> {
    let names: Vec<&str> = ids.iter().map(String::as_str).collect();
    let bodies = git::read_objects(repo_git, Some(repo_git), None, &names)
        .context("cannot read the commits in the task repo")?;
    ids.iter()
        .zip(&bodies)
        .map(|(id, raw)| parse_commit(id, raw))
        .collect()
}

fn parse_commit(id: &str, raw: &[u8]) -> Result<Original> {
    let sep = raw
        .windows(2)
        .position(|w| w == b"\n\n")
        .with_context(|| format!("commit {id} has no header/message separator"))?;
    let message = raw[sep + 2..].to_vec();

    let mut tree = None;
    let mut parents = Vec::new();
    let mut header_lines = Vec::new();
    let mut in_signature = false;
    for line in raw[..sep].split(|&b| b == b'\n') {
        if line.starts_with(b" ") {
            if !in_signature {
                header_lines.push(line.to_vec());
            }
            continue;
        }
        in_signature = line.starts_with(b"gpgsig ") || line.starts_with(b"gpgsig-sha256 ");
        if in_signature {
            continue;
        }
        if let Some(v) = line.strip_prefix(b"tree ") {
            tree = Some(object_id(id, v)?);
        } else if let Some(v) = line.strip_prefix(b"parent ") {
            parents.push(object_id(id, v)?);
        }
        header_lines.push(line.to_vec());
    }
    let tree = tree.with_context(|| format!("commit {id} has no tree header"))?;
    Ok(Original {
        tree,
        parents,
        header_lines,
        message,
        raw: raw.to_vec(),
    })
}

/// The object id in a `tree` or `parent` header value of commit `id`: git writes it as ASCII
/// hex, so anything else means the commit is damaged.
fn object_id(id: &str, value: &[u8]) -> Result<String> {
    match std::str::from_utf8(value) {
        Ok(v) if !v.is_empty() && v.bytes().all(|b| b.is_ascii_hexdigit()) => Ok(v.to_owned()),
        _ => bail!("commit {id} has a malformed tree or parent header"),
    }
}

/// Defangs `message` (raw commit-message bytes, not necessarily valid UTF-8 -- issue 244's
/// encoding edge case) the way [`defang`] defangs a `&str`, without altering a byte [`defang`]
/// never touches. [`defang`] only ever *inserts* a zero-width space right after an existing
/// character; it never changes or drops one. So: decode `message` into a `String` good enough for
/// [`defang`] to see word boundaries in (each maximal invalid byte run standing in for one
/// placeholder character, which is never alphanumeric, `@`, `#`, `:` or whitespace -- none of the
/// characters [`defang`] treats as a trigger), remembering each resulting character's exact byte
/// range in `message`; run the unchanged function on that `String`; then walk its result against
/// the original character sequence and reassemble the final bytes from `message` itself for every
/// character kept, and from the inserted character's own UTF-8 bytes for every one `defang` added.
fn defang_bytes(message: &[u8]) -> Vec<u8> {
    let mut decoded = String::new();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut pos = 0;
    while pos < message.len() {
        match std::str::from_utf8(&message[pos..]) {
            Ok(valid) => {
                push_chars(valid, pos, &mut decoded, &mut spans);
                pos = message.len();
            }
            Err(e) => {
                let valid_len = e.valid_up_to();
                if valid_len > 0 {
                    let valid = std::str::from_utf8(&message[pos..pos + valid_len])
                        .expect("valid_up_to names a valid prefix");
                    push_chars(valid, pos, &mut decoded, &mut spans);
                }
                let bad_start = pos + valid_len;
                let bad_len = e.error_len().unwrap_or(message.len() - bad_start).max(1);
                decoded.push('\u{fffd}');
                spans.push((bad_start, bad_start + bad_len));
                pos = bad_start + bad_len;
            }
        }
    }

    let defanged = defang(&decoded);
    let original_chars: Vec<char> = decoded.chars().collect();
    let mut out = Vec::with_capacity(message.len());
    let mut next = 0;
    for ch in defanged.chars() {
        if next < original_chars.len() && ch == original_chars[next] {
            let (start, end) = spans[next];
            out.extend_from_slice(&message[start..end]);
            next += 1;
        } else {
            let mut buf = [0u8; 4];
            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        }
    }
    out
}

/// Pushes every character of `text` (a valid-UTF-8 piece of a larger byte buffer that starts at
/// `base` in that buffer) onto `decoded`, recording each one's exact byte range in `spans` for
/// [`defang_bytes`] to reassemble from later.
fn push_chars(text: &str, base: usize, decoded: &mut String, spans: &mut Vec<(usize, usize)>) {
    let mut offset = base;
    for ch in text.chars() {
        let len = ch.len_utf8();
        spans.push((offset, offset + len));
        decoded.push(ch);
        offset += len;
    }
}

/// Builds a new commit object with `original`'s header -- every line verbatim except a `parent`
/// line, replaced in place with the matching entry of `parents` (already rebuilt where they
/// needed to be, in the same order `original.parents`/`header_lines` has them) -- and `message`,
/// and writes it (`git hash-object -t commit -w --stdin`); returns its id. Nothing is written to
/// any ref here.
///
/// Built and hashed directly, rather than through `git commit-tree -F <file>`: `commit-tree`
/// quietly reencodes a message that isn't valid UTF-8 (issue 244's encoding edge case) even when
/// it is given verbatim from a file, where `hash-object` stores exactly the bytes it is given.
/// `commit-tree` also checks that the tree and every parent exist before it writes anything;
/// [`check_range`] does that check instead, once for the whole range.
fn build_commit(
    repo_git: &Path,
    id: &str,
    original: &Original,
    parents: &[String],
    message: &[u8],
) -> Result<String> {
    let mut object = Vec::new();
    let mut rebuilt_parents = parents.iter();
    for line in &original.header_lines {
        if line.starts_with(b"parent ") {
            let parent = rebuilt_parents
                .next()
                .expect("one rebuilt parent per original `parent` header line");
            object.extend_from_slice(format!("parent {parent}\n").as_bytes());
        } else {
            object.extend_from_slice(line);
            object.push(b'\n');
        }
    }
    object.push(b'\n');
    object.extend_from_slice(message);
    // Nothing to defang, no parent rebuilt, no signature dropped: the rebuilt commit is the
    // original, which the repo already holds (issue #444).
    if object == original.raw {
        return Ok(id.to_owned());
    }

    let out = git::run_with_stdin(
        repo_git,
        Some(repo_git),
        None,
        &["hash-object", "-t", "commit", "-w", "--stdin"],
        &object,
    )?;
    Ok(out.trim().to_owned())
}

/// Moves the delivered branch `branch` to `new_tip` and [`rebuilt_marker`] to `new_marker`
/// (this rebuild's agent tip) together, as one atomic `git update-ref --stdin` transaction: both
/// move, or neither does. Each carries its own compare-and-swap (`old_tip` for the branch,
/// `old_marker` for the marker, which is created instead when `None`), so a mismatch on either
/// -- something else having moved the branch, or a concurrent rebuild having moved the marker --
/// refuses the whole transaction and changes nothing; so does the agent ref no longer being at
/// `new_marker`, the tip that was rebuilt.
fn move_branch(
    repo_git: &Path,
    branch: &str,
    new_tip: &str,
    old_tip: &str,
    marker: &str,
    old_marker: Option<&str>,
    new_marker: &str,
) -> Result<()> {
    let delivered = format!("refs/heads/{branch}");
    // The agent ref must still be at the tip that was rebuilt: read before a fetch, it is checked
    // here, in the same transaction (issue #444).
    let mut stdin = format!(
        "verify {} {new_marker}\nupdate {delivered} {new_tip} {old_tip}\n",
        agent_ref(branch)
    );
    match old_marker {
        Some(old) => stdin.push_str(&format!("update {marker} {new_marker} {old}\n")),
        None => stdin.push_str(&format!("create {marker} {new_marker}\n")),
    }
    let out = git::output_with_stdin(
        repo_git,
        Some(repo_git),
        None,
        &["-c", &no_hooks_config(repo_git)?, "update-ref", "--stdin"],
        stdin.as_bytes(),
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

    /// Like [`git`], but the raw stdout bytes, for a test that must tell a real byte apart from
    /// one `String::from_utf8_lossy` replaced.
    fn git_bytes(dir: &Path, args: &[&str]) -> Vec<u8> {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
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

        rebuild(&dir, "refs/heads/main", "issue-4").unwrap();

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

        rebuild(&dir, "refs/heads/main", "issue-4").unwrap();

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

        rebuild(&dir, "refs/heads/main", "issue-4").unwrap();

        let rewritten = git(&dir, &["rev-parse", "refs/heads/issue-4"]);
        let message = git(&dir, &["log", "-1", "--format=%B", &rewritten]);
        assert_eq!(
            message,
            "héllo @\u{200b}bob 日本語 that also closes\u{200b} #7"
        );
    }

    #[test]
    fn a_non_utf8_message_byte_survives_the_rebuild_unchanged() {
        let (_tmp, dir) = repo();
        let empty_tree = git(&dir, &["hash-object", "-t", "tree", "--stdin"]);
        let base = git(&dir, &["commit-tree", &empty_tree, "-m", "base"]);
        git(&dir, &["update-ref", "refs/heads/main", &base]);
        // `encoding ISO-8859-1` and a message byte (0xE9, "é") that isn't valid UTF-8 on its own:
        // git never requires or converts this, so a worker's commit can carry it as-is.
        let mut raw = Vec::new();
        raw.extend_from_slice(format!("tree {empty_tree}\nparent {base}\n").as_bytes());
        raw.extend_from_slice(
            b"author t <t@t> 1700000000 +0000\ncommitter t <t@t> 1700000000 +0000\n\
              encoding ISO-8859-1\n\n",
        );
        raw.extend_from_slice(b"caf\xe9 fixes #12\n");
        let agent = write_commit_object(&dir, &raw);
        git(&dir, &["update-ref", "refs/sbxm/agent/issue-4", &agent]);
        git(&dir, &["update-ref", "refs/heads/issue-4", &base]);

        rebuild(&dir, "refs/heads/main", "issue-4").unwrap();

        let rewritten = git(&dir, &["rev-parse", "refs/heads/issue-4"]);
        let full = git_bytes(&dir, &["cat-file", "-p", &rewritten]);
        let sep = full
            .windows(2)
            .position(|w| w == b"\n\n")
            .expect("header/message separator");
        let message = &full[sep + 2..];
        let mut expected = b"caf\xe9 fixes".to_vec();
        expected.extend_from_slice("\u{200b}".as_bytes());
        expected.extend_from_slice(b" #12\n");
        assert_eq!(message, expected.as_slice(), "{message:?}");
    }

    /// Review round 2, M-3: issue 244 requires the delivered branch to equal the agent's own
    /// commit, id for id, whenever nothing in its message needs a change. A non-signature header
    /// other than the ones `rebuild` happens to look at by name (`encoding`, say) must survive
    /// too, or that guarantee breaks for every commit that carries one.
    #[test]
    fn an_unchanged_message_with_an_encoding_header_keeps_its_exact_id() {
        let (_tmp, dir) = repo();
        let empty_tree = git(&dir, &["hash-object", "-t", "tree", "--stdin"]);
        let base = git(&dir, &["commit-tree", &empty_tree, "-m", "base"]);
        git(&dir, &["update-ref", "refs/heads/main", &base]);
        git(&dir, &["update-ref", "refs/heads/issue-4", &base]);
        // No closing reference or mention anywhere, so `defang` has nothing to change.
        let mut raw = Vec::new();
        raw.extend_from_slice(format!("tree {empty_tree}\nparent {base}\n").as_bytes());
        raw.extend_from_slice(
            b"author t <t@t> 1700000000 +0000\ncommitter t <t@t> 1700000000 +0000\n\
              encoding ISO-8859-1\n\n",
        );
        raw.extend_from_slice(b"caf\xe9 plain message\n");
        let agent = write_commit_object(&dir, &raw);
        git(&dir, &["update-ref", "refs/sbxm/agent/issue-4", &agent]);

        rebuild(&dir, "refs/heads/main", "issue-4").unwrap();

        assert_eq!(git(&dir, &["rev-parse", "refs/heads/issue-4"]), agent);
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

        rebuild(&dir, "refs/heads/main", "issue-4").unwrap();

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

    /// Review round 2, M-2: a merge's second parent can already be reachable from a branch that
    /// has nothing to do with this task (one already on GitHub, or another task's own). That
    /// commit is not the agent's to rewrite -- only a commit reachable *only* through the agent
    /// ref is -- so the delivered merge must keep its exact id, closing reference and all.
    #[test]
    fn a_merge_parent_already_on_another_branch_keeps_its_exact_id() {
        let (_tmp, dir) = repo();
        let empty_tree = git(&dir, &["hash-object", "-t", "tree", "--stdin"]);
        let base = git(&dir, &["commit-tree", &empty_tree, "-m", "base"]);
        git(&dir, &["update-ref", "refs/heads/main", &base]);
        git(&dir, &["update-ref", "refs/heads/issue-4", &base]);
        let existing = git(
            &dir,
            &[
                "commit-tree",
                &empty_tree,
                "-p",
                &base,
                "-m",
                "existing fixes #8",
            ],
        );
        // Already on another host branch before the agent ever touched this task -- not the
        // agent's commit to rewrite, even though the agent's merge reaches it.
        git(&dir, &["update-ref", "refs/heads/other-branch", &existing]);
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
        let merge = git(
            &dir,
            &[
                "commit-tree",
                &empty_tree,
                "-p",
                &main_line,
                "-p",
                &existing,
                "-m",
                "merge side, fixes #9",
            ],
        );
        git(&dir, &["update-ref", "refs/sbxm/agent/issue-4", &merge]);

        rebuild(&dir, "refs/heads/main", "issue-4").unwrap();

        let rewritten = git(&dir, &["rev-parse", "refs/heads/issue-4"]);
        assert_ne!(rewritten, merge);
        let p1 = git(&dir, &["rev-parse", &format!("{rewritten}^1")]);
        let p2 = git(&dir, &["rev-parse", &format!("{rewritten}^2")]);
        assert_eq!(p1, main_line);
        assert_eq!(
            p2, existing,
            "the pre-existing commit must keep its exact id"
        );
        let existing_message = git(&dir, &["log", "-1", "--format=%B", &p2]);
        assert_eq!(existing_message, "existing fixes #8");
    }

    /// Review round 4, M-1: the worker's branch can fast-forward from the task base to a commit
    /// already reachable from another branch, leaving nothing of the agent's own to rebuild. The
    /// delivered branch must land on that commit's exact id, not stay at the base, and a later
    /// rebuild over the same agent tip must not take it for a branch moved by something else.
    #[test]
    fn a_fast_forward_to_an_existing_branch_keeps_that_tip() {
        let (_tmp, dir) = repo();
        let empty_tree = git(&dir, &["hash-object", "-t", "tree", "--stdin"]);
        let base = git(&dir, &["commit-tree", &empty_tree, "-m", "base"]);
        git(&dir, &["update-ref", "refs/heads/main", &base]);
        git(&dir, &["update-ref", "refs/heads/issue-4", &base]);
        let existing = git(
            &dir,
            &[
                "commit-tree",
                &empty_tree,
                "-p",
                &base,
                "-m",
                "existing fixes #8",
            ],
        );
        git(&dir, &["update-ref", "refs/heads/other-branch", &existing]);
        git(&dir, &["update-ref", "refs/sbxm/agent/issue-4", &existing]);

        rebuild(&dir, "refs/heads/main", "issue-4").unwrap();
        assert_eq!(git(&dir, &["rev-parse", "refs/heads/issue-4"]), existing);

        rebuild(&dir, "refs/heads/main", "issue-4").unwrap();
        assert_eq!(git(&dir, &["rev-parse", "refs/heads/issue-4"]), existing);
    }

    /// Review round 4, M-2: a header byte that isn't valid UTF-8 (an author name written in
    /// ISO-8859-1) must survive byte for byte, so a commit whose message needs no change keeps its
    /// exact id.
    #[test]
    fn unchanged_non_utf8_header_bytes_keep_the_commit_id() {
        let (_tmp, dir) = repo();
        let empty_tree = git(&dir, &["hash-object", "-t", "tree", "--stdin"]);
        let base = git(&dir, &["commit-tree", &empty_tree, "-m", "base"]);
        git(&dir, &["update-ref", "refs/heads/main", &base]);
        git(&dir, &["update-ref", "refs/heads/issue-4", &base]);
        let mut raw = Vec::new();
        raw.extend_from_slice(format!("tree {empty_tree}\nparent {base}\n").as_bytes());
        raw.extend_from_slice(
            b"author Ren\xe9 <t@t> 1700000000 +0000\ncommitter t <t@t> 1700000000 +0000\n\n",
        );
        raw.extend_from_slice(b"a plain message\n");
        let agent = write_commit_object(&dir, &raw);
        git(&dir, &["update-ref", "refs/sbxm/agent/issue-4", &agent]);

        rebuild(&dir, "refs/heads/main", "issue-4").unwrap();

        assert_eq!(git(&dir, &["rev-parse", "refs/heads/issue-4"]), agent);
    }

    /// Review round 4, M-2, second case: when the message does need defanging, every header byte
    /// still stays exact; only the message changes.
    #[test]
    fn non_utf8_header_bytes_survive_a_message_rewrite() {
        let (_tmp, dir) = repo();
        let empty_tree = git(&dir, &["hash-object", "-t", "tree", "--stdin"]);
        let base = git(&dir, &["commit-tree", &empty_tree, "-m", "base"]);
        git(&dir, &["update-ref", "refs/heads/main", &base]);
        git(&dir, &["update-ref", "refs/heads/issue-4", &base]);
        let header = format!("tree {empty_tree}\nparent {base}\n").into_bytes();
        let mut header = header;
        header.extend_from_slice(
            b"author Ren\xe9 <t@t> 1700000000 +0000\ncommitter t <t@t> 1700000000 +0000",
        );
        let mut raw = header.clone();
        raw.extend_from_slice(b"\n\nthis fixes #12\n");
        let agent = write_commit_object(&dir, &raw);
        git(&dir, &["update-ref", "refs/sbxm/agent/issue-4", &agent]);

        rebuild(&dir, "refs/heads/main", "issue-4").unwrap();

        let rewritten = git(&dir, &["rev-parse", "refs/heads/issue-4"]);
        assert_ne!(rewritten, agent);
        let full = git_bytes(&dir, &["cat-file", "-p", &rewritten]);
        let sep = full
            .windows(2)
            .position(|w| w == b"\n\n")
            .expect("header/message separator");
        assert_eq!(&full[..sep], header.as_slice());
    }

    /// PR 250 review, M-1: a highly compressible message can be tiny in the bundle and huge once
    /// read, and rebuilding holds several copies of it. A commit object over [`MAX_COMMIT_BYTES`]
    /// is refused before it is read, and the delivered branch stays where it was.
    #[test]
    fn a_commit_over_the_size_cap_is_refused_before_it_is_read() {
        let (_tmp, dir) = repo();
        let empty_tree = git(&dir, &["hash-object", "-t", "tree", "--stdin"]);
        let base = git(&dir, &["commit-tree", &empty_tree, "-m", "base"]);
        git(&dir, &["update-ref", "refs/heads/main", &base]);
        git(&dir, &["update-ref", "refs/heads/issue-4", &base]);
        let header = format!(
            "tree {empty_tree}\nparent {base}\nauthor t <t@t> 1700000000 +0000\ncommitter t <t@t> 1700000000 +0000\n\n"
        );
        let mut raw = header.into_bytes();
        let fill = MAX_COMMIT_BYTES as usize + 1 - raw.len();
        raw.resize(raw.len() + fill, b'a');
        let agent = write_commit_object(&dir, &raw);
        git(&dir, &["update-ref", "refs/sbxm/agent/issue-4", &agent]);

        let err = format!(
            "{:#}",
            rebuild(&dir, "refs/heads/main", "issue-4").unwrap_err()
        );

        assert!(err.contains(&agent), "{err}");
        assert_eq!(git(&dir, &["rev-parse", "refs/heads/issue-4"]), base);
    }

    #[test]
    fn a_commit_of_exactly_the_size_cap_is_rebuilt() {
        let (_tmp, dir) = repo();
        let empty_tree = git(&dir, &["hash-object", "-t", "tree", "--stdin"]);
        let base = git(&dir, &["commit-tree", &empty_tree, "-m", "base"]);
        git(&dir, &["update-ref", "refs/heads/main", &base]);
        git(&dir, &["update-ref", "refs/heads/issue-4", &base]);
        let header = format!(
            "tree {empty_tree}\nparent {base}\nauthor t <t@t> 1700000000 +0000\ncommitter t <t@t> 1700000000 +0000\n\n"
        );
        let mut raw = header.into_bytes();
        let fill = MAX_COMMIT_BYTES as usize - raw.len();
        raw.resize(raw.len() + fill, b'a');
        let agent = write_commit_object(&dir, &raw);
        git(&dir, &["update-ref", "refs/sbxm/agent/issue-4", &agent]);

        rebuild(&dir, "refs/heads/main", "issue-4").unwrap();

        assert_eq!(git(&dir, &["rev-parse", "refs/heads/issue-4"]), agent);
    }

    /// Crafts a commit object by hand (`git hash-object -t commit -w --stdin`), so a fake
    /// signature or a non-UTF-8 message byte can be used without a real GPG key or losing bytes
    /// to a `&str`'s own UTF-8 requirement.
    fn write_commit_object(dir: &Path, content: &[u8]) -> String {
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
        child.stdin.take().unwrap().write_all(content).unwrap();
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
        let agent = write_commit_object(&dir, raw.as_bytes());
        git(&dir, &["update-ref", "refs/sbxm/agent/issue-4", &agent]);
        git(&dir, &["update-ref", "refs/heads/issue-4", &base]);

        rebuild(&dir, "refs/heads/main", "issue-4").unwrap();

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
            format!(
                "tree {bogus_tree}\nparent {c1}\nauthor t <t@t> 1700000000 +0000\ncommitter t <t@t> 1700000000 +0000\n\nsecond\n"
            )
            .as_bytes(),
        );
        let c3 = git(
            &dir,
            &["commit-tree", &empty_tree, "-p", &c2, "-m", "third"],
        );
        git(&dir, &["update-ref", "refs/sbxm/agent/issue-4", &c3]);

        let err = format!(
            "{:#}",
            rebuild(&dir, "refs/heads/main", "issue-4").unwrap_err()
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

        rebuild(&dir, "refs/heads/main", "issue-4").unwrap();
        let first_tip = git(&dir, &["rev-parse", "refs/heads/issue-4"]);
        rebuild(&dir, "refs/heads/main", "issue-4").unwrap();
        let second_tip = git(&dir, &["rev-parse", "refs/heads/issue-4"]);

        assert_ne!(first_tip, agent);
        assert_eq!(first_tip, second_tip);
    }

    /// [`move_branch`]'s compare-and-swap is `rebuild`'s only way to move the delivered branch, so
    /// this is also the backstop for two rebuilds racing on the same task (decision: the record's
    /// own running check, tested elsewhere, is the first line of defence; this is what still holds
    /// if it is ever bypassed).
    /// `rev` doesn't resolve to anything in `dir` (a ref was never created, or a transaction that
    /// should have created or moved it was refused and rolled back).
    fn ref_missing(dir: &Path, rev: &str) -> bool {
        let out = Command::new("git")
            .current_dir(dir)
            .args(["rev-parse", "--verify", "--quiet", rev])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        !out.status.success()
    }

    #[test]
    fn move_branch_refuses_when_the_branch_is_not_at_the_expected_old_value() {
        let (_tmp, dir) = repo();
        let empty_tree = git(&dir, &["hash-object", "-t", "tree", "--stdin"]);
        let base = git(&dir, &["commit-tree", &empty_tree, "-m", "base"]);
        let other = git(&dir, &["commit-tree", &empty_tree, "-m", "elsewhere"]);
        let new = git(&dir, &["commit-tree", &empty_tree, "-m", "new"]);
        git(&dir, &["update-ref", "refs/heads/issue-4", &other]);
        git(&dir, &["update-ref", "refs/sbxm/agent/issue-4", &new]);

        let message = format!(
            "{:#}",
            move_branch(
                &dir,
                "issue-4",
                &new,
                &base,
                "refs/sbxm/rebuilt/issue-4",
                None,
                &new,
            )
            .unwrap_err()
        );

        assert!(message.contains("issue-4"), "{message}");
        assert_eq!(git(&dir, &["rev-parse", "refs/heads/issue-4"]), other);
        // The marker is created only together with the branch move: a refused branch update
        // must leave it absent too, not created on its own.
        assert!(ref_missing(&dir, "refs/sbxm/rebuilt/issue-4"));
    }

    /// Issue 244's required case, distinct from the CAS test above: a branch moved **since the
    /// last rebuild**, not merely during this call. The first rebuild has no earlier marker to
    /// compare against, so it always goes through; the second rebuild must recognise that the
    /// branch it now finds isn't what its own previous rebuild left there, and refuse rather
    /// than overwriting it.
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
        rebuild(&dir, "refs/heads/main", "issue-4").unwrap();

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
            rebuild(&dir, "refs/heads/main", "issue-4").unwrap_err()
        );

        assert!(err.contains("issue-4"), "{err}");
        assert_eq!(git(&dir, &["rev-parse", "refs/heads/issue-4"]), elsewhere);
        assert_eq!(git(&dir, &["rev-parse", "refs/sbxm/agent/issue-4"]), second);
    }

    /// Review round 2, M-1: `fetch_bundle` advances the agent ref *before* `rebuild` runs, so a
    /// rebuild that then fails still leaves the agent ref ahead of whatever was last actually
    /// rebuilt. The next collection must not mistake that gap for the delivered branch having
    /// been moved by something else: it should rebuild successfully, over the full range, and
    /// land on the same ids a clean run over that same range would.
    #[test]
    fn a_rebuild_that_never_ran_after_a_fetch_does_not_block_the_next_one() {
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
        rebuild(&dir, "refs/heads/main", "issue-4").unwrap();

        // A later bundle fetch advances the agent ref further, but the rebuild that should have
        // followed it never ran (crashed, or failed for a reason that has since gone away) --
        // nothing else touched the delivered branch.
        let second = git(
            &dir,
            &["commit-tree", &empty_tree, "-p", &first, "-m", "second"],
        );
        git(&dir, &["update-ref", "refs/sbxm/agent/issue-4", &second]);

        rebuild(&dir, "refs/heads/main", "issue-4").unwrap();

        let rewritten = git(&dir, &["rev-parse", "refs/heads/issue-4"]);

        // A clean run over the same two commits, from scratch, must give the same final id.
        let (_tmp2, clean_dir) = repo();
        let clean_base = git(&clean_dir, &["commit-tree", &empty_tree, "-m", "base"]);
        assert_eq!(clean_base, base);
        git(&clean_dir, &["update-ref", "refs/heads/main", &clean_base]);
        git(
            &clean_dir,
            &["update-ref", "refs/heads/issue-4", &clean_base],
        );
        let clean_first = git(
            &clean_dir,
            &["commit-tree", &empty_tree, "-p", &clean_base, "-m", "first"],
        );
        let clean_second = git(
            &clean_dir,
            &[
                "commit-tree",
                &empty_tree,
                "-p",
                &clean_first,
                "-m",
                "second",
            ],
        );
        assert_eq!(clean_second, second);
        git(
            &clean_dir,
            &["update-ref", "refs/sbxm/agent/issue-4", &clean_second],
        );
        rebuild(&clean_dir, "refs/heads/main", "issue-4").unwrap();

        assert_eq!(
            rewritten,
            git(&clean_dir, &["rev-parse", "refs/heads/issue-4"])
        );
    }

    // PR #482 review round (M-1): the agent ref is read before the fetch, so the move pins it.

    #[test]
    fn move_branch_refuses_when_the_agent_ref_is_not_at_the_rebuilt_tip() {
        let (_tmp, dir) = repo();
        let empty_tree = git(&dir, &["hash-object", "-t", "tree", "--stdin"]);
        let base = git(&dir, &["commit-tree", &empty_tree, "-m", "base"]);
        let new = git(&dir, &["commit-tree", &empty_tree, "-m", "new"]);
        let later = git(&dir, &["commit-tree", &empty_tree, "-m", "later"]);
        git(&dir, &["update-ref", "refs/heads/issue-4", &base]);
        git(&dir, &["update-ref", "refs/sbxm/agent/issue-4", &later]);

        move_branch(
            &dir,
            "issue-4",
            &new,
            &base,
            "refs/sbxm/rebuilt/issue-4",
            None,
            &new,
        )
        .unwrap_err();

        assert_eq!(git(&dir, &["rev-parse", "refs/heads/issue-4"]), base);
        assert!(ref_missing(&dir, "refs/sbxm/rebuilt/issue-4"));
    }

    #[test]
    fn the_range_tells_the_count_only_when_it_sits_on_the_start_alone() {
        let range = |boundary: &[&str], commits: &[(&str, &[&str])]| Range {
            commits: commits
                .iter()
                .map(|(id, parents)| {
                    (
                        (*id).to_owned(),
                        parents.iter().map(|p| (*p).to_owned()).collect(),
                    )
                })
                .collect(),
            boundary: boundary.iter().map(|b| (*b).to_owned()).collect(),
        };
        let rebuilt = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
            pairs
                .iter()
                .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
                .collect()
        };
        let linear = range(&["s"], &[("a", &["s"]), ("b", &["a"])]);
        let two = rebuilt(&[("a", "a2"), ("b", "b2")]);

        assert_eq!(ahead_from_range(&linear, "s", &two), Some(2));
        // Another branch's commit under the range, a root commit, two commits rebuilt into one,
        // and an empty range: each is counted instead.
        let merged = range(&["s", "o"], &[("a", &["s"]), ("b", &["a", "o"])]);
        assert_eq!(ahead_from_range(&merged, "s", &two), None);
        let rooted = range(&["s"], &[("a", &[]), ("b", &["a", "s"])]);
        assert_eq!(ahead_from_range(&rooted, "s", &two), None);
        let same = rebuilt(&[("a", "x"), ("b", "x")]);
        assert_eq!(ahead_from_range(&linear, "s", &same), None);
        assert_eq!(ahead_from_range(&range(&[], &[]), "s", &rebuilt(&[])), None);
    }
}
