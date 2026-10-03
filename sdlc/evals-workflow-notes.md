# Evals milestone: workflow notes (started 2026-10-02)

Working through the deferred evaluators (cosine, spike S9, then Jev) with `sbxm task` wherever possible. This file
records, as it happens, what sbxm could not do and which steps a small script (a `just` recipe or a `sbxm` command)
could define. Each entry says what happened, the workaround used, and the candidate fix. Nothing here is decided.

## Missing functionality in sbxm

| # | Gap | Evidence | Workaround used | Candidate |
|---|---|---|---|---|
| G1 | **A task can't get extra egress.** Building `fastembed` needs `cdn.pyke.io` (ONNX Runtime, sha256 pinned in `ort-sys`'s `dist.tsv`); the gates compile every crate, so a worker or a gate run in a sandbox without it fails. Profiles are shared and deny-by-default. | cosine issue #63 | Ask the user to allow the host in `sbxm-dev` | `[sandbox] extra_allow = ["host", ...]` in `sbxm-task.toml` (reviewed like any config, recorded in the kit hash), or a per-issue profile via `--profile` |
| G2 | **Default Rust gates fail on a Windows host** whenever the workspace is a host mount: linking a test binary into it gave `Permission denied`. | PR 57 review runs 1-2 | Local `sbxm-task.toml` with `CARGO_TARGET_DIR=/tmp/target` on every cargo gate | `task init` and the built-in Rust default should put the target dir in the sandbox (`/tmp/target`) |
| G3 | **A gate's output is cut to its last 8 KB**, so a failing test's panic text is hidden behind the "Running ..." lines. | PR 57 round 2 gate log | A gate command that greps failures to the end | Keep the full output in a file next to `gates.log` and put failure sections in the tail |
| G4 | **`sbxm task` blocks**; no detach, wait or progress command. | every review run | `Start-Process` with log files, then polling | `--detach` plus `task wait`, or `task logs --follow` |
| G5 | **The local review config is not part of the repo** (`sbxm-task.toml` with the machine's gates and the E2E config dir), so each run is: build, copy binary and config in, set `SBXM_CONFIG_DIR`, `task rm`, start, clean up. | PR 57 and 62 | Copy by hand | A `just review-pr <n>` recipe that does exactly that |
| G6 | **TLS interception breaks Rust crates with bundled roots.** Norton's SSL scanning re-signs HTTPS; the OS store trusts it, `fastembed`'s downloader does not. | cosine probe | Fetch model files with `curl`, load locally | `sbxm doctor` could probe one HTTPS host and name the issuer when it isn't a public CA |
| G7 | **No place for downloaded assets.** `<config dir>` holds `config.toml` and profiles only. | cosine design (decision 166) | `model_dir` option, files fetched by hand | `sbxm models fetch` (a follow-up of #63) and a `models/` folder in the config dir |
| G8 | **Sandbox disk lives on C:, `doctor` doesn't check it.** A review sandbox takes about 7 GB there while it exists; the machine ran out twice. | PR 57 runs | Freed space by hand | `sbxm doctor` reads the drive of Docker's sandbox state and warns under about 10 GB |
| G9 | **Integration tests that need the host or the network can't run in a task sandbox** (a real model, real `sbx`). They are `#[ignore]`d and nobody runs them in a loop. | cosine design | Ignored tests with an env var, run by hand | `just real-test` already exists for `sbx`; add `just model-test` and list both in the PR template |
| G10 | **Unix-only code can't be tested from the Windows host.** The process-group kill and the symlink tests ran only through a WSL clone. | PR 57 M-2 | `git fetch` into a WSL clone, `cargo test` there | A `just test-linux` recipe doing that fetch-and-test |
| G11 | **Custom secrets can't be expressed in a profile or checked.** `sbx secret set-custom` is host-side only, `preflight` and `doctor` don't know about it, and `sbxm rm --purge` leaves sandbox-scoped custom secrets behind. | spike S9 (PR #65) | Created and removed by hand with `sbx` | A `[secrets.custom]` table naming env var and host (never a value), checked by preflight and `doctor`, and removed with the sandbox |
| G12 | **A throwaway sandbox with one extra egress host needs a hand-written `sandbox.toml`** before `sbxm new`. | spike S9 | Wrote the file by hand | `sbxm new --allow-host <h>` or a `just scratch` recipe |
| G13 | **Removing a custom secret is error-prone:** it needs `--placeholder` and the right `--sandbox`, a positional argument is read as a service name, and `--all` deletes everything. Proving real secrets were untouched was a habit, not a command. | spike S9 | Snapshot of `sbx secret ls --json` before and after | A script that snapshots, runs a command, diffs and refuses on any change to non-throwaway secrets |
| G14 | **No `sbxm exec`.** Every probe went through `sbx exec`, which adds "started successfully" lines to stdout. | spike S9 | Filtered the noise | `sbxm exec <project> --harness h -- <cmd>` with clean output |
| G15 | **Two same-env-name custom secrets (global and scoped) make `sbx create` fail** with `400 invalid custom secrets`. | spike S9 | Removed one | Preflight should refuse a clash before any sandbox is created |
| G16 | **BLOCKS THE LOOP: no way to keep working on an open PR with `task`.** `task review --pr` has no fix round; `task start` always begins from a base branch (`--base issue-63` might work, untried); a `ready` task refuses `task review --issue` ("already ready; the review is review.md") and `task gates` ("gates run after the worker or after the fix round"); a second PR review needs `task rm --pr N` first. Review, fix and re-review on one branch is therefore manual. | PR 69: the PR review found 2 must-fix findings (M-1, M-2) and nothing in `task` could act on them | Fixed by hand on the `issue-63` branch, test first | `task fix --pr N` (or `start --pr N`) giving a worker the PR head and the review; a re-review that replaces or extends the old task (`review-2`); an explicit flag to push to an existing PR branch (today only `finish` pushes, and it opens a new PR). **Needs a numbered decision through the planning interview first (next session).** |
| G17 | **A fix round needs the worker's original sandbox and never recreates it.** The sandbox `sbxm-task-issue-63-claude` was gone when the fix round started: `error: sandbox ... not found`, task `fixing/failed`, and `check_can_review` then refuses `fixing/failed` ("the worker failed"), so there is no resume. Why the sandbox vanished is unknown. | issue-63 run 1 (2026-10-02) | `task start --restart` (a whole new worker run) | Recreate the sandbox from the saved kits for a fix round, or always use a fresh one; and allow `review` to retry a `fixing/failed` task |
| G18 | **A full sandbox disk looks like a code failure.** `cargo test` exited 101 with an empty-looking gate output because the gate only greps `FAILED|panicked`; the cause (`No space left on device (os error 28)`, then `ld terminated with signal 7`) was only in `/tmp/t.log` inside the sandbox, which is deleted with the sandbox. Nothing checks disk before the gates. The three debug builds (check, clippy, test) shared one `/tmp/target` that reached 17 GB on a 20 GB disk. | issue-63 gates (twice) and PR 69 review run 3 | Read `/tmp/t.log` in the kept worker sandbox; later `CARGO_PROFILE_DEV_DEBUG=0` and a gate command that also prints `^error`, `No space` and `df` (local, uncommitted `sbxm-task.toml`) | `gates.log` should keep the tail of the real output; a pre-gate disk check; the built-in Rust default gates should shrink the build. The `debug=0` effect is **not proven**: one pass after three failures, with more free space at the time |
| G19 | **The sandbox's own disk lives on the system drive and `sbx` has no setting for it.** `%LOCALAPPDATA%\DockerSandboxes\sandboxes\state` held 40.2 GB (nominal) for one task sandbox: a 20 GB root layer (`rwlayer.img`), a 10 GB Docker volume and about 2.5 GB of images. `base_dir` only moves the workspace. `sbx settings list` (v0.46.0) has `sandbox.disk.dockerVolume` (a size) and nothing for location. C: fell to 0 GB three times on 2026-10-02, and the `sbx` daemon then answered 401. `doctor` checks the base dir's free space only. | issue-63 worker; spike S11 (PR 67) | Cleared space by hand; deleted `/tmp/target` (frees space in the sandbox, not on the host, because the image does not shrink) | `doctor` should check the drive `sbx` keeps its state on; document the per-sandbox cost; a junction for the state folder is untested; templates (S11) would not shrink build output |
| G20 | **Gates run in different environments for the worker and for a PR review.** The issue-63 worker ran `apt-get install pkg-config` itself (`dpkg.log` 20:10), so its gates passed; the clean review sandbox had `libssl-dev` from the image but no `pkg-config`, so `openssl-sys` failed (PR 69 attempt 1). The user's rule: workers must not install unsanctioned software. | PR 69 | Added `pkg-config libssl-dev` to the `sbxm-dev` install step (PR 70, merged) | Templates with every dependency baked in and a strict mode that blocks updates (own milestone, see "Next session"); until then, run the gates once in a fresh sandbox before the review |
| G21 | **Nothing records which `sbxm` build ran a step.** `sbxm --version` prints `sbxm 0.1.0` for every build; `task.json` has no commit, exe path or hash. The installed `sbxm` and the copy used for the run differed (hash, size, build time) and nobody could say which commit either came from, so the results of that run were set aside and redone with a recorded build. | issue-63 run 2 | `H:\sbxm-e2e\build-known.ps1`: a `--locked` release build of a clean commit, a manifest (commit, rustc, SHA-256) and a hash line at the top of every log | `--version` should include the git commit (and dirty flag); `task.json` should record the version, commit and the invoking command line for every stage |
| G22 | **Tasks are visible only under the config dir and base dir they were created with.** A plain `sbxm task review --issue 63` answered `no task issue-63` because `SBXM_CONFIG_DIR` pointed elsewhere. | issue-63 | Set `SBXM_CONFIG_DIR=H:\sbxm-e2e\config` | The error should name the config dir and base dir it looked in |
| G23 | **`deploy-profiles` replaces whole profiles from the current branch and cannot see local-only changes.** I deployed from a branch based on `main` and overwrote the deployed copy of the still-unmerged PR 66 (`cdn.pyke.io`), so PR 69 attempt 2 failed on a 403 for `ort-sys`. | PR 69 | Deployed a throwaway local merge of PR 66 and PR 70, then PR 66 was merged | Show a diff and ask first, or refuse unless on an up-to-date `main`; `--check` mode |
| G24 | **No guard against two operators on one task.** Steps on `issue-63` were run from the user's terminal (gates at 21:31, a second review at 21:38) while the session also ran steps, with an unrecorded binary. `task.json` keeps the process id of the run, not who or what ran it. | issue-63 | Asked to run it from one place | Record the command line and exe per stage (with G21); a lock held for the whole stage |
| G26 | **Two `task_*` tests are flaky under load.** In one full host `cargo test` run (everything running in parallel) `tests/task_finish.rs::a_record_that_names_a_hostile_branch_is_refused` failed with "task issue-41 is failed in stage working, so it cannot move on" and `tests/task_review_cmd.rs::failing_gates_fail_the_command_and_no_reviewer_runs` with "the worker failed for task issue-41"; both passed twice when run alone. The common thing is a worker recorded as failed in test setup; a timing or process-probe race is a guess, not verified. | PR 69 host gate | Reran the two files alone | Find the race (the tests fake a worker through the real process probe); until then a gate failure in these two should be rerun alone before it is believed |
| G25 | **A `ready` task can't be re-gated or re-reviewed, and the only exit is a full restart (about 1.5 hours).** Part of G16, listed because it also bit when the second review had been run with an unknown binary. | issue-63 | Opened the PR with `task finish` and used the PR review as the clean review | `task review --issue N --again` that keeps the commits |
| G27 | **DEFERRED (just in time, the user's call 2026-10-03): task identifiers carry no project.** A task is `issue-<n>` or `pr-<n>`: its record folder (`<base>/.sbxm/tasks/issue-63`), its clones (`<base>/tasks/issue-63`) and its sandboxes (`sbxm-task-issue-63-claude`) carry no repo or project. Tasks are defined per repo, so a **separate `base_dir` per repo keeps the folders apart**. What is shared is the machine's `sbx` sandbox list: a test with the smallest sandbox type showed that creating a second sandbox with an existing name is refused (`error: sandbox '<name>' already exists`), so two repos that both start issue 63 would collide at sandbox creation even with separate base dirs. Not tested through `sbxm task` itself. Nobody is hitting this today (one repo is in use); fix it when a second repo is used. | factory architecture draft; `sbx create` test | none needed yet | Put the project in the sandbox name (and the id) when a second repo appears; refuse a name that exists with a clear message |

## Friction in the working environment (not sbxm's code)

- `git` on Windows intermittently answers `Permission denied` on `.git/objects` (an indexer or antivirus holding a
  file); `git add` can stop halfway and a following `git commit` then commits a partial index. A wrapper that
  retries and refuses to commit when the add failed would remove a real way to lose work.
- The bash hook that blocks `\\` stops any command containing a Rust line continuation in a string. Edits with
  such strings go through the Write tool or a script file.
- Long test runs hit the 10 minute tool timeout and move to the background; a recipe that always runs them in the
  background and reports once would be calmer.
- Files `sbxm` writes are UTF-8 without a BOM. Windows PowerShell 5.1 and some editors read them as ANSI, so the
  check mark `✓` showed as `âœ“` in a log the user was following. `Get-Content -Encoding utf8` or PowerShell 7
  reads them correctly; a BOM would be a cheap fix for logs written to disk.
- `sbx exec` on a stopped sandbox starts it again ("started successfully"), so a read-only look at the kept worker
  sandbox restarted it each time and it had to be stopped again.
- The assistant's file tools can't read outside the working directories (`E:\james\...\sbxm` and
  `E:\sbxm-projects`), so a worktree on `H:` had to be moved under `E:\sbxm-projects`. Cargo output can stay on `H:`
  with `CARGO_TARGET_DIR`.
- A long unattended run (about 25 minutes per review, 1.5 hours for a whole task) is easy to start twice, once by
  hand and once from the session. Starting one from the session with a log file that begins with the exe and hash
  made the state checkable (G21, G24).

## Run log: issue 63 and PR 69, 2026-10-02 to 2026-10-03

What ran, in order, so the next session does not have to reconstruct it.

| # | Run | Result |
|---|---|---|
| 1 | `task start --restart --issue 63` with the unrecorded `sbxm-63.exe`, then `task review` (codex) | Worker: 4 commits, gates passed. Review 1: 3 must-fix (M-1 silent embedding failure, M-2 cosine labels differ from the judge's, M-3 no two-repeat isolation test). Fix round failed: the worker's sandbox was gone (G17). Set aside: binary unknown (G21). |
| 2 | Same again, with `sbxm-f243288.exe` (release, `--locked`, commit `f243288`, SHA-256 `F56CB5EB...D736838`) | Worker: 8 commits, gates passed. Review 1: 2 must-fix (M-1 failures recorded silently, M-2 cosine-enabled repeats vanish from the output) plus S-1 (download size not documented). Fix round ran; the gates then failed on a full sandbox disk (G18). The gates and `review-2` were rerun from the user's terminal with another binary (G21, G24): 0 must-fix, but unrecorded. |
| 3 | `task finish --issue 63` (pinned exe) opened PR 69; `task review --pr 69` attempts 1 to 4 | 1: `openssl-sys` needs `pkg-config` (G20; PR 70). 2: `ort-sys` got 403 from `cdn.pyke.io` because my deploy had overwritten PR 66 (G23). 3: `cargo test` failed with no readable reason (G18). 4: gates passed; review: 2 must-fix, posted on PR 69. |
| 4 | By hand on `issue-63` (worktree `E:\sbxm-projects\wt-63`, host cargo), test first | Commit `fc6ec59` on PR 69. M-1: a single answer must still reach the embedder so a model that cannot embed is recorded (two existing tests that asserted "no embedder call" were changed, three tests failed first). M-2: one warning for different runtime errors, each repeat keeps its own (one command test, failed first). Host gate: `fmt` and `clippy` exit 0; in the full `cargo test` run `task_finish` and `task_review_cmd` each had one failure ("the worker failed for task issue-41") that passed twice in isolation, so flaky under load (G26). **Not yet re-reviewed:** PR 69 needs a fresh independent review with a recorded binary (`task rm --pr 69`, then `task review --pr 69`). |

Observations. The two worker runs of the same issue gave 4 and 8 commits and different review findings; only the
silent-embedding-failure finding appears in both (as M-1 in run 1 and run 2). A single review is therefore a sample,
not a verdict. The C: drive fell to 0 GB on 2026-10-02 and was at 9 to 14 GB for the rest of the session.

PRs from this session: 66 (allow `cdn.pyke.io`, merged), 67 (spike S11 docs, open), 68 (S9 fake placeholders,
merged), 69 (cosine, open), 70 (`pkg-config`, merged), 71 (`docs/workflow.md`). Spike S9's earlier text had
realistic-looking sandbox placeholders; they were never credentials and were replaced by obvious fakes. The old
text is still in git history (commit `bf3cb2b`); rewriting `main` was not done.

## Next session (agenda, in this order)

1. **Interview: continuing work on an open PR (G16, G17, G25).** Questions to settle, each with a recommendation:
   shape (`task fix --pr`, a fix round in `task review --pr`, or `task start --pr`); who pushes to an existing PR
   branch and behind which flag; whether a re-review replaces or extends the old task; recreate or always use a
   fresh sandbox for a fix round. Output: a numbered decision in `sdlc/decisions.md`, issues with acceptance
   criteria, then the work goes through the workflow as a milestone batch.
2. **Interview: templates and a strict mode (G20, G19).** Every dependency baked into a template, and sandboxes
   blocked from updating themselves (no apt, rustup or crates hosts, a pre-warmed cargo cache, `--offline`, a changed
   `Cargo.lock` fails loudly). Finish spike S11 first (PR 67; it needs 20 GB free on C:). Open questions: what
   "blocking updates" covers (the Claude kit runs its own `apt-get update` at startup), where and how often a
   template is built, and strict as an opt-in profile flag with today's behavior as the default.
3. **Small, decision-free fixes that can be issues now:** G18 (keep the real gate output, a pre-gate disk check),
   G21 and G24 (record version, commit and command per stage), G22 (name the config dir in the error), G23
   (`deploy-profiles` shows a diff first), G19's `doctor` check, a BOM for logs, and finding the flaky tests (G26).
4. **Cosine (issue 63) status:** PR 69 needs a fresh independent review of the fix commits with a recorded binary,
   then the user's merge. Jev (slice 14a) starts after that; the token rule in the handoff still applies.

## Where a script could define the process

1. **Start a milestone slice from an issue:** file the issue (from a template with the acceptance-criteria
   checklist), `task start`, `task review`, `task finish`, `task rm`. Everything but the issue text is a fixed
   sequence: a `just slice <issue>` recipe.
2. **Review a PR:** G5's `just review-pr <n>`.
3. **Spikes:** a throwaway project plus an extra egress host plus a cleanup that proves the real secrets are
   untouched (S9 was the first such spike, PR #65): a `scripts/spike-sandbox.ps1` that takes the baseline,
   creates the project, runs a probe, searches the sandbox for the secret, cleans up and diffs.
4. **After a review:** copy `review.md` into `sdlc/reviews/` with prefixed ids, file the should-fix findings,
   write the resolution line. Done by hand three times so far (PR 57 rounds 1-3, PR 62); `task file-findings`
   covers only the filing.
