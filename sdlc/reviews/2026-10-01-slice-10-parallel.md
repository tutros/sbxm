# Review: M2b slice 10 (parallel review, decision 162)

Scope: `43aefed..58ab9b8` (slice 10, 6 commits by session B), reviewed in the background by an independent subagent
while slice 9 was being built. Result: **2 must-fix**, 5 should-fix, 2 questions. The reviewer read the code and did
not run cargo, so findings rest on reading. Issues: none filed yet. The must-fix findings are fixed on the milestone
branch (see "Status"); the rest are collected for the end-of-milestone review.

## Must-fix

**M1. `start --restart` deletes the task before start's own selection checks, so it can leave the user with nothing**
(`src/commands/task_start.rs`, `discard_existing` before `pipeline::select_issues`). `discard_existing` checked only
that the issue is open and the task isn't running, but selection can still skip the issue as a question or blocked, so
"nothing to start" was reported after the task, clones and sandboxes were already gone. The same applied to
`task run --restart`. A smaller form: with two `--issue`s, the user confirmed the first deletion, declined the second,
and the first was already deleted. Fix (done): `pipeline::check_restartable` runs the selection rules (open, not a
question, not blocked, not related to another task; the tasks being restarted don't count as "has a task") before
the first deletion; `finish::discard_many` lists every task and asks once for all of them.

**M2. `finish` pushes agent-authored commits unfiltered, including `.github/workflows/**`** (`repo::push`,
`finish::finish`). A same-repo branch push triggers `on: push` / `pull_request` workflows with the repository's
secrets before any human has reviewed anything; the agent's commits are untrusted (decision 127). No guard exists.
Fix: `finish` refuses before pushing when the task branch changes `.github/workflows/**` or `.github/actions/**`
relative to the base, naming the files, with no `pr_create` call (a bypass would be an explicit flag and a decision).

## Should-fix

3. **PR body carries unsanitised agent text** (`finish::pr_body`): a `Fixes #12`, `Closes owner/other#3` or `@someone`
   inside result.md or review.md closes other issues on merge or pings people. Neutralise closing keywords and
   @-mentions, or wrap both files in a fenced block or `<details>`.
4. **The secret scan is thin** (`findings::secret_kind` through `refuse_secrets`): misses Slack `xox[bp]-`, Google
   `AIza...`, npm `npm_`, JWTs, Stripe `sk_live_`, `api_key:` / `key=` forms and `Authorization: Basic`; and it scans
   only result.md and review.md, not the pushed diff (say so in `--help` or a decision). Table-driven tests, then
   patterns.
5. **A failure after the push can leave a stuck task**: if `gh pr create` created the PR but the call failed, the
   re-run is refused ("a pull request already exists") and the task is never marked finished. Look for an open PR for
   the head branch before `pr_create` and adopt its URL (needs a `GitHubBackend` method such as `pr_for_branch`).
6. **A non-fast-forward push rejection gets the wrong hint** (`repo::push`): the context says "check `gh auth status`"
   for a branch that already exists on origin with different history.
7. **`rm` on a task whose `task.json` can't be read skips the running check**: a live worker's folders could be
   deleted after a confirmation. Require `--yes` plus a printed warning, or refuse when a pid/lock exists.

## Questions

8. `check_own_folder` and `remove_dir_all` on Windows: case, 8.3 short-name and trailing-dot differences make
   `canonicalize` differ from the expected path, so a legitimate removal is refused (fails safe) when `base_dir` is
   configured with different casing. Untested: a case-mismatched `base_dir`, long paths. The check-then-delete gap is
   acceptable for a user-run CLI.
9. No non-terminal test that `run --restart` without `--yes` fails before any deletion (`finish::discard` does handle
   it through `confirm.is_interactive()`).

## Checked and fine

Id validation before any path is built (`is_valid_id`, ids formed from `u32`); the sandbox-name check
(`sbxm-task-<id>-` prefix makes `issue-1` vs `issue-10` safe; a hand-edited `task.json` cannot name another task's
sandbox); removal order (sandboxes, clones, `task.json` last, kept if anything else stayed, non-zero exit);
confirmation shows exactly what is deleted, no terminal and no `--yes` refuses, "no" deletes nothing, `--yes` is
enforced by clap `requires`; running tasks and newer schemas are refused; `finish` requires `ready`/ok, no existing
PR, kind issue; `check_repo`, `valid_ref_name` on branch and base and `commits_ahead > 0` run before the push; the push
is non-forcing with an explicit refspec to `origin`; secret scan fails before any push; `read_capped` refuses
non-regular files; the 25,000-character cap is announced; `task run` stops at `ready` and never pushes or calls
`pr_create`; its reviewer flags are checked before the worker starts.

## Status

M1 fixed (commit "Slice 10 review must-fix 1"). M2 fixed in the next commit. Should-fix 3 to 7 and questions 8, 9
collected for the end-of-milestone review.
