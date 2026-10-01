# Review: M2b slice 5 (parallel review, decision 162)

Scope: `874b2e8..4cac339` (slice 5, 4 commits), reviewed in the background by an independent subagent while slice 6
was next. Result: 0 must-fix, 4 should-fix, 2 questions. Core boundary holds: no host git command gets the
workspace as cwd, `--git-dir` or `--work-tree` in `fetch_bundle`. `cargo test --test task_repo --test task_kits`
passed (reviewer ran it). Issues: none filed yet; collect these into the end-of-milestone review.

## Should-fix

**S1. Bundle size cap and "plain file" check can be raced or bypassed** (`src/task/repo.rs`, `fetch_bundle`).
`symlink_metadata` checks only the last path component, then `std::fs::copy` re-resolves the path and follows links.
The agent can swap in a link between check and copy, or keep appending while the copy runs, so the cap is enforced
only after the whole file is on host disk. A parent such as `<ws>/.sbxm-task` can itself be a link. Impact: DoS and a
confidentiality edge (the copy must still pass `git bundle verify`), not code execution.
Fix (TDD): (1) a file larger than the cap is refused with the copy never exceeding cap+1 bytes; open once, check
`file.metadata()` on the handle, copy through `io::copy(&mut file.take(cap + 1), ...)`. (2) a bundle whose parent dir
is a link is refused; canonicalize the parent and require it to stay inside the workspace.

**S2. `fetch_pr_head` fetches a PR author's objects with no fsck** (`src/task/repo.rs`). `fetch_bundle` sets
`transfer.fsckObjects` and `fetch.fsckObjects`; the PR path does not. Fix (TDD): a PR head with a bad `.gitmodules`
entry is refused; add both `-c` options to that fetch.

**S3. Non-fast-forward detection matches English stderr text** (`fetch_bundle`). Another locale, or another
"rejected" message (hook, fsck), gives the wrong error. Fix: check ancestry first with `merge-base --is-ancestor`
between the current tip and the bundle tip (or force `LC_ALL=C`). The existing rewritten-history test still guards it.

**S4. The planted-command tests can pass vacuously** (`tests/task_repo.rs`). No positive control proves the sentinel
command works on this host; `hostile_committed_content_...` never triggers a checkout. Fix (TDD): a control that runs
plain `git status` in the hostile workspace and sees the sentinel appear; a checkout of the fetched branch with the
hardened runner for the committed-content case.
Evidence already gathered by the author: deleting the `core.hooksPath` setting made `hooks_in_repo_git_do_not_run_on_the_fetch_path`
fail ("a hook ran during the fetch"), so that test does catch a hook; the config and committed-content tests still lack a control.

## Questions

**Q1.** `user_output` applies the user's global git config (for example `filter.lfs.*`) when cloning a workspace that
contains agent-committed `.gitattributes`. A committed `* filter=lfs` would run the user's LFS smudge program on the
host at the workspace clone. Recommendation: acceptable for now; the later gate slices should check agent content out
with the hardened runner. To decide with the user.

**Q2.** The 500 MB cap covers the compressed bundle only; unpacked size and the `fetch` duration are unbounded.
Recommendation: accept, it is the stated requirement.

## Checked and fine

Ref-name and refspec injection (`valid_ref_name`, `--`, refspecs from validated names or a `u32`); the hardened
environment of `fetch_bundle` (`--git-dir` is `repo.git`, no global/system config, no inherited `GIT_*`,
`protocol.allow=never` with only `protocol.file.allow=always`, fsck on, no tags or submodules, `gc.auto=0`); only the
named ref is imported, fast-forward only; verify and fetch read the host-owned copy, so there is no gap between them;
`push` never forces and sends one ref; `clone_workspace` uses `--no-hardlinks --template=` from a host-owned parent;
`kits::build_for` is a pure refactor.
