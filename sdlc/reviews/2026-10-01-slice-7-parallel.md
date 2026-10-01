# Review: M2b slice 7 (parallel review, decision 162)

Scope: `1af34d8..21a65cf` (slice 7, 5 commits), reviewed in the background by an independent subagent while
slice 8 was next. Result: 0 must-fix, 5 should-fix, 2 questions. Nothing makes the host tier worse than decision 160
documents; the weak spots are leftover state, a missing lock and process cleanup, not code execution. The reviewer ran
the gate and record test binaries (all pass); it did not run fmt/clippy or the ignored real-`sbx` tests, and could not
run a hostile-repo experiment (the worktree guard blocked its compound git command), so the clean-checkout conclusions
rest on reading the code and git's documented defaults; long-path behavior is untested. Issues: none filed yet;
collect these into the end-of-milestone review.

## Should-fix

**G7-1. A leftover `<id>-gates` folder fails every later host run and is reported as a gate failure**
(`src/task/pipeline.rs` host tier, `src/task/repo.rs` `clean_checkout`). `clean_checkout` refuses an existing `dest`,
`run_gates` turns that into a failed `(clean checkout)` gate. The folder stays behind when sbxm is killed during a host
gate, or `remove_with_retries` gives up (a detached process keeps the folder as its cwd on Windows). The path comes only
from a validated id, so removing it first is safe. Fix (TDD): a `run_gates` test that plants `issue-41-gates/stale.txt`
and expects a pass with the stale file gone; remove the folder before `clean_checkout`; if removal fails, record a note
and fail with a message that is not a gate failure.

**G7-2. An interrupted or half-written gating run is a dead end** (`check_can_gate`, `run_gates`, `begin_gating`).
`(Gating, Running)` is always refused, even when the process is dead; a crash, or a failing `gates.log` or final record
write (`?` after the work ran), leaves the record `Running` with a dead pid and nothing but `start --restart` (a later
slice, and it covers `start`, not `gates`) gets out. Fix (TDD): `check_can_gate` and `begin_gating` accept Gating/Running
when `is_interrupted`; a `run_gates` test where `gates.log` is a directory must not leave the record `Running` (make the
log write best-effort with a note, or write the record first).

**G7-3. The timeout kill is incomplete off Windows, and output is lost when something else holds the pipe**
(`src/task/gates.rs` `kill_tree`, `drain`, `ShellHostRunner::run`). Off Windows `kill_tree` is only `child.kill()`, so
what `sh` started survives (the test hides this because `sh -c "sleep 30"` execs directly). `drain` sends its buffer only
at EOF: a surviving grandchild holding the pipe makes `recv_timeout(3s)` return nothing, so the failing or timed-out
command's tail is lost. On Windows `taskkill /T` runs only on timeout, so a detached child can outlive a normal exit
and keep the checkout locked (feeds G7-1). Fix (TDD): a unix-only test (`sleep 30 & wait`, 1 s timeout) asserting the
process is gone; run in a new process group and kill the group; publish output through a shared buffer so the timeout
path returns what was captured; a Windows test with a detached `ping` asserting earlier output is kept.

**G7-4. No exclusion between two runs on the same task** (`task_gates.rs`, `run_gates`, `record.rs`). Two
`task gates --issue 5` runs both read `Passed`, both write `Running`, share the workspace and the `issue-5-gates`
folder, race on `gates.log` and `record.gates`, and the last `record::write` wins; `begin_gating`'s guard only protects
an in-memory record. Fix (TDD): an exclusive lock file `<meta>/gates.lock` created with `create_new`, refusing a second
run, with stale-lock handling through `is_interrupted`.

**G7-5. A committed `cargo.cmd` or `cargo.exe` in the checkout root can replace the tool** (`host_command`, Windows).
`cmd.exe` searches the current directory before `PATH` unless `NoDefaultCurrentDirectoryInExePath` is set (from cmd's
documented behavior; not run). Inside decision 160's accepted risk, but a one-line environment variable removes the
surprise. Fix (TDD): extend the `host_commands_never_inherit_git_variables` unit test so `host_command(...)` sets
`NoDefaultCurrentDirectoryInExePath=1` on Windows, then set it.

## Questions

**Q1. Host-gate environment: inherit everything except `GIT_*`, or an allowlist?** Today tokens such as `GITHUB_TOKEN`
or `ANTHROPIC_API_KEY` reach agent code on the host, and an 8 KB output tail lands in `gates.log` (nothing here uploads
it). Reviewer recommends keeping the inheritance (an allowlist breaks real builds: `CARGO_HOME`, `RUSTUP_HOME`,
`USERPROFILE`, MSVC variables; and agent code can read `~/.git-credentials`, the `gh` token file and `~/.ssh` anyway),
adding a line to `task gates --dry-run` ("inherits your environment, including tokens"), and a rule for the review/PR
slices that `gates.log` is never posted to GitHub automatically. To decide with the user.

**Q2. A newline in a gate command.** `sh -c` runs both lines, `cmd /C` stops at the first, so a second line is silently
dropped on Windows (cmd behavior, not run). `task/config.rs` doesn't validate gate strings. Recommendation: reject a
gate command containing a line break at load, naming the file (decision 11: no silent drop).

## Checked and fine

Which commands run (config comes from the user's cwd `sbxm-task.toml`; the agent's branch is never read for config;
nit: `task gates` doesn't check the cwd repo is the task's repo); shell parsing (`cmd /D /S /C` with `raw_arg`, `sh -c`
as one argv element, no splicing of checkout or environment text); the clean checkout (ref checks and `--`, `--template=`,
no user or system config, only `protocol.file` allowed, no recursion, `.gitattributes` filters have no driver,
`core.protectNTFS` default); cleanup path derivation and `remove_dir_all` not following junctions (std behavior, not
tested here); state-machine order (stage written before any gate runs, re-gating, `check_can_gate` messages, failed
worker not gated, timed-out worker gated, `--tier host` alone); result handling (first failure stops, exit codes,
timed-out, output capped to 8 KB per command, `--dry-run` writes nothing); the README and starter warnings; tests are
not vacuous (the fake records cwd and command, the hook test inspects the checkout while a gate "runs"). Minor: `record.gates`
grows without bound across re-runs.
