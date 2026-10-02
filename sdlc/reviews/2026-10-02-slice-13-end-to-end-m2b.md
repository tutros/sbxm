# Slice 13 (M2b): end-to-end check of `sbxm task` (2026-10-02)

Run from `m2b-implementation` at `769411b` (slices 0-12), `sbx` 0.46.0, `gh` logged in as `tutros`, secrets `anthropic`,
`openai` and `google` stored. Scratch repo: `tutros/sbxm-scratch` (private, created for this check; a small Rust
crate, `sbxm-task.toml`, labels `must-fix`/`should-fix`/`question`, issues #1-#10). The base dir was
`H:\sbxm-e2e\base` through a separate `SBXM_CONFIG_DIR` (E: and C: were nearly full; the real config was not touched).
Real agent runs: Claude, Codex and Antigravity as workers and as reviewers.

| Checklist item | Result | Evidence |
|---|---|---|
| 1. `task init`; `task gates --dry-run` lists the commands | Pass, with S-5 | Starter file valid; dry run lists the three cargo commands, host tier off. The command needs `--issue N` |
| 2. `task start --workers 2`: picks, parallel sandboxes, `status` shows `working` | Pass, with S-2 and S-6 | Two workers ran in parallel (issues 1 and 7) after `gh auth setup-git`; `status` showed `working`, then `gating`. Skip reasons print only for issues passed over before the cap |
| 3. Kill one `sbxm` mid-run | Pass | `status`: `working running`, then `working interrupted`; `start` refuses ("already has a task"); `--restart --yes` removed sandbox and folders and the new worker completed |
| 4. Deliberate gate failure | Pass | Issue 5 (a `1 == 2` test): `gates-failed`, exit 101 in the gate, `start` exited 1, stopped |
| 5. Review with Claude/Codex/Antigravity as worker and reviewer | Pass, fix round not exercised | Claude worker + Codex reviewer (issues 1, 7, 10), Codex worker + Claude reviewer (issue 8), Antigravity worker + Antigravity reviewer (issue 9); every review wrote `Reviewer: <harness> (default model)` and `Must-fix findings: <n>`; reviewer sandboxes removed. All reviews had 0 must-fix, so the one fix round was **not** exercised (unit tests only) |
| 6. `review --pr N` posts a comment; fork PR refused | Partly | PR #6 with a failing gate: refused, nothing posted. PR #6 with a gate-passing change: comment posted (1 must-fix, `first` panics on an empty slice), reviewer sandbox removed. **Fork refusal not run** (needs a second account) |
| 7. `task finish`; `file-findings` dry run then `--create` | Pass, with S-3 | `finish` pushed `issue-1`, opened PR #11 with `Fixes #1`, result and review in the body. `file-findings --file` dry run, `--create` filed #12-#14 with links in both directions, rewrote `Issues:`; a rerun skipped all three |
| 8. `task rm` shows paths and confirms; nothing left | Pass, with S-7 | Without a terminal and without `--yes`: refusal listing exact sandboxes and paths, nothing removed. After `--yes` on all tasks: `sbx ls` empty, base dir only empty containers |
| 9. No token, secret or run-config text readable in a task sandbox; no git run in an agent's clone | Pass on what could be checked | No credential files, no hooks/filters in the clone's `.git/config`, GitHub returns 403 from inside. `GH_TOKEN` *is* set in the sandbox: a 40-character `gho_` value that is **not** the host's token (different hash) and gets 403 from the API; it looks like an `sbx` placeholder, but that was not proven (O-1) |

## Findings

### Must-fix

**M-1 (fixed) - Sandbox gates run with a PATH that has no `~/.cargo/bin`.** `task start` on the first worker ended
`gates-failed` with `cargo fmt --check: exit 127, sh: 1: cargo: not found`, while the worker had run `cargo test`
itself. Rust is installed by the `sbxm-dev` profile into `~/.cargo`; `sbx exec ... sh -c` doesn't source it. So the
default gates for a Rust repo, as `task init` writes them, fail out of the box. Worked around in the scratch repo by
prefixing `. "$HOME/.cargo/env" &&`. Seen in `src/task/gates.rs` (`sh -c`); `src/run/checks.rs` uses the same pattern.

**M-2 (fixed) - A credential prompt hangs `task start` forever.** Host network git (`git clone --bare https://...`) used the
user's git config; with the Git Credential Manager unable to prompt, a clone sat on VS Code's askpass for 10+ minutes
with no output, and `sbxm` stayed hung after the git processes were killed. The orphaned askpass helper then held the
task folder open, so `task rm` and `--restart` failed ("used by another process") until it was killed. It happened with
parallel `--workers 2` clones (the second one) and as "Cannot prompt because user interactivity has been disabled"
on single clones. Fixed on this machine by `gh auth setup-git`. Needed: `GIT_TERMINAL_PROMPT=0` (and no inherited
askpass) on network git, a timeout, and a clear error naming `gh auth setup-git`.

### Should-fix

**S-1 (not run) - the one fix round.** No review found a must-fix finding on an issue task, so
fix prompt, second gates and second review were not exercised for real.

**S-2 - `task start` leaves no record during sandbox setup.** For the first ~3 minutes `status` says "No tasks yet"
while a sandbox is already running; `task.json` appears only when the worker starts. A task killed before that
leaves folders with no record: `start` then says "stage no readable record" and `--restart` is the way out (works).

**S-3 - `file-findings --issue/--pr` can't read a task's `review.md`.** The reviewer writes `Reviewer:`, `Must-fix
findings: <n>` and free-form findings; `file-findings` demands the skill format with one `Issues:` line and refuses
("issue-7-review.md has no 'Issues:' line(s)"). The README says `--issue`/`--pr` take the task's `review.md`. Either
the reviewer prompt asks for the skill format, or `file-findings` accepts the task format, or the README is changed.
The parallel-review records under `sdlc/reviews/` hit the same refusal.

**S-4 - `start` for an issue whose branch already exists on origin fails with a raw git error:**
`git branch -- issue-1 refs/heads/main failed: a branch named 'issue-1' already exists` (after `finish` had pushed it).
Should say that the branch exists on origin and what to do.

**S-5 - `task init` prints `sbxm task gates --dry-run`**, which needs `--issue N`; checklist item 1 has the same wording.

**S-6 - `start --workers N` prints skip reasons only for issues passed over before the cap.** Checklist item 2 says
"every skip reason"; spec rule 6 stops at the cap, so either is fine, but one of them should change.

**S-7 - `task rm` prints `error: sandbox '...' not found` for sandboxes that are already gone** (reviewer
sandboxes are removed after each round), and then `removed sandbox ...`. Cosmetic but alarming.

**S-8 - README says `--issue` "starts exactly the issues named"**, but a named issue that is blocked by an open issue is
still refused ("blocked by open #2"), as the spec says.

### Observations

- **O-1** `GH_TOKEN` placeholder in sandboxes (item 9): confirm with `sbx` what it is.
- The worker's sandbox stays `running` after a gate failure but is `stopped` after a pass (seen once; cosmetic).
- Sandbox setup takes about 3 minutes per task (apt, rustup, `cargo install just`); sandboxes are created one after
  another, so `--workers 2` overlaps the agent runs but not the setup (see the sandbox-template-prebuild idea).
- C: filling up (0.9 GB free) made a Codex reviewer's sandbox fail with `fsync` I/O errors and a read-only file system;
  `sbxm doctor` checks the base dir's free space but not the drive Docker's sandbox disks live on.
- Out of scope per the PRD (not tested): tasks without an issue or PR, and local repos without GitHub. The user
  asked for a way to run a task that has no source code or no issue/PR; that needs a decision (PRD non-goal).

## Not done

- Fork PR refusal (item 6): needs a second GitHub account; covered by `FakeGitHub` tests only.
- The `#[ignore]` real-`sbx` suite was not run.
- Scratch repo `tutros/sbxm-scratch` remains (private) with PR #6, PR #11, issues #1-#14 and branches `pr-demo`, `issue-1`.
