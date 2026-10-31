# sbxm

`sbxm` creates and manages per-project Docker Sandboxes (`sbx`) from one
shared, versioned config. Each project gets a folder on your machine, and each coding agent (Claude Code, Codex,
Gemini CLI, Pi or Antigravity) gets its own sandbox for it, built with your network allowlist, environment, secrets, instructions
and setup steps.

sbxm enforces nothing itself: `sbx` does the isolation, the deny-by-default egress proxy and secret injection. sbxm
turns your config into `sbx` kits, passes them to `sbx create`, and remembers what each sandbox was built from, so it
can tell you when the config has changed since.

For how the commands fit together (sandboxes, comparisons, working on GitHub issues), with use cases, see
[`docs/workflow.md`](docs/workflow.md). This file is the reference.

## Requirements

- [Docker Desktop](https://www.docker.com/products/docker-desktop/) with `sbx` **0.43.0 or newer**, logged in
  (`sbx login`). Prebuilt templates (see *Templates*) need `sbx` **0.46.0 or newer**; that minimum only applies to
  a profile with `[template]`. `sbxm task` sandboxes in Docker Cloud Sandboxes also need `sbx` **0.46.0 or newer**,
  logged in to the cloud; that minimum only applies to a cloud task (see *Running tasks in Docker Cloud Sandboxes*).
- The secrets your agents need, stored in `sbx` (sbxm only names them): for example `sbx secret set anthropic`.
- [Rust](https://rustup.rs/) to build sbxm. [`just`](https://github.com/casey/just) is optional, for the recipes in
  the `justfile`.
- A base folder for projects that `sbx` can mount. Not under `%TEMP%` or `AppData`. On the machine sbxm was built on,
  workspaces on `C:` failed to mount, so projects live on `E:`.

sbxm is developed on Windows 10 with PowerShell; paths are handled portably, but other platforms are untested.

## Install

```
cargo install --path .
```

or `just install`. Without installing, run it from this folder as `cargo run -- <command>` (the `justfile` recipes do
that).

## Quick start

1. Write a starter config and a `default` profile:
   ```
   sbxm config init
   ```
   This creates `~/.config/sbxm/config.toml` and `~/.config/sbxm/profiles/default/profile.toml`
   (`%USERPROFILE%\.config\sbxm` on Windows; set `SBXM_CONFIG_DIR` to use another folder). It never overwrites.
2. Open `config.toml` and set `base_dir` to the folder your projects should live in, e.g. `'E:\sbxm-projects'`.
   Create that folder.
3. Check everything:
   ```
   sbxm doctor
   ```
   Every line should start with `ok`. Each `FAIL` line says what's wrong and how to fix it.
4. Create a project and its Claude Code sandbox, then attach to it:
   ```
   sbxm new demo
   sbxm open demo --harness claude
   ```
   The workspace is `<base_dir>\demo`, mounted read-write into the sandbox.

## Commands

| Command | What it does |
|---|---|
| `sbxm config init` | Writes a starter `config.toml` and `default` profile. Refuses to overwrite either. |
| `sbxm new <project> [--harness h] [--profile p] [--seed dir]` | Creates `<base_dir>/<project>` if missing (or reuses it), builds the kits from the profile and the project's `sandbox.toml`, checks them with `sbx kit validate`, and creates the sandbox. Doesn't attach. Refuses a harness that already has a sandbox for the project, before writing anything; open that one with `sbxm open <project> --harness h` (or `--rebuild` it). `--seed` copies a folder into a *new* project. |
| `sbxm open <project> --harness h [--rebuild]` | Attaches to the sandbox, starting it if it's stopped and creating it if it's missing. Refuses if the config changed since the sandbox was built; `--rebuild` recreates it. |
| `sbxm list [--json]` | Lists sbxm's sandboxes with project, harness, status and whether their config is `current` or `changed`. Flags orphans (a sandbox sbxm has no record of, or a record without a sandbox) and says how to fix each. |
| `sbxm stop <project> --harness h` | Stops the sandbox. |
| `sbxm rm <project> --harness h` | Removes the sandbox and sbxm's record of it. The workspace is kept. |
| `sbxm rm <project> --purge [--yes]` | Removes **every** sandbox of the project, then deletes the workspace and its metadata, after you confirm the exact paths. Without a terminal it needs `--yes`. Can't be combined with `--harness`. |
| `sbxm run init [path]` | Writes a starter run-config for a comparison (default `./run.toml`): two contestants (Claude and Codex) live, an Antigravity one commented out, and commented examples for checks, a rubric and a judge. The file is valid as written. Refuses to overwrite. |
| `sbxm run <config>` | Runs a comparison: checks the run-config and that every needed secret is stored in `sbx` (each contestant's provider, the judge's, and the profiles' `secrets.services`), builds and validates the kits, then gives each (contestant, repeat) pair its own throwaway sandbox. A contestant can set its own `profile`; otherwise `[run].profile` (or your `default_profile`) applies, and the judge uses the run's. Contestants of a repeat run in parallel; repeats run one after another. Each sandbox is removed afterwards, even after an error; the workspaces stay under `<base_dir>/runs/<run-id>/<contestant>/<repeat>/`. Prints the run ID and one line per pair, naming the contestant as `<harness> (<model>)` (completed, timed out or failed). Everything still prints and saves the same way whether or not a pair errored; only at the very end, if any pair's line says `error` (sbxm couldn't run the contestant at all: seeding, sandbox creation or the exec itself, as opposed to one that ran and timed out or whose harness failed), the run fails: `Error: <n> of <m> contestant runs ended in error` on stderr and exit code 1, like any failing command (decision 181). With `task.seed`, each contestant gets its own copy of the seed as a fresh git repo with one baseline commit (the seed's history and remotes are dropped); without it, an empty plain folder. Each pair's diff (everything the contestant changed or added, even if it committed) is captured before its sandbox is removed. Each pair's results are saved the moment it finishes under `<base_dir>\.sbxm\runs\<run-id>\<contestant>\<repeat>\` (`answer.md`, `diff.patch`, `transcript.jsonl`, `result.json`), next to `run.json` (the run's identity: config hashes, profile, `sbx` version, times, and for each (profile, harness) pair the exact template tag its sandboxes were created from, or `null` when they were built with the install steps) and a copy of the run-config; `run.json` gets `completed_at` only once everything is saved. `run.log` in the same folder is a copy of what the run printed, screen output and warnings alike (decision 170): the lines printed before the folder existed, then every later one, and a failed run's final error worded as on screen, each stamped with the time and the part of the run it came from (`preflight`, `pairs`, `judge`, `cosine`, `jev` or `results`), under the same header line as a task's `run.log` (the command line, the `sbxm` executable and its SHA-256, the commit if the build knows it, the process id); values that look like API keys or tokens are masked, and it does not hold the `sbx` child process's own output. If it can't be written, the run goes on and prints one `warning: cannot write <path>: <cause>`. Any `[[eval.checks]]` run inside each contestant's sandbox after its agent finishes (and after its diff is taken), as `sh -c <command>` with a time limit enforced inside the sandbox; exit code 0 passes. The verdicts are saved in `evals.json` next to the pair's other files and summarised on the pair's line (`; checks 2/3 passed`). With `[eval.judge]` and a rubric, an LLM judge then scores the contestants: after all pairs are done, once per repeat, in its own throwaway sandbox, it sees each contestant's answer and diff under an anonymous label (A, B, ...) and scores every rubric criterion (`pass_fail` or a `scale` of levels). A judge from the same provider as a contestant is allowed, with a warning. Verdicts are saved in each pair's `evals.json` and per repeat in `judge/<repeat>/judge.json` (the label mapping); a judge that fails is warned about and doesn't fail the run. The contestants are then ranked: each criterion is 0 to 1, a contestant's score is the weighted mean over the criteria the judge scored (criteria it skipped are listed, never counted as 0), repeats are averaged, equal scores share a rank, and executable checks are shown alongside without changing the score. The ranking is printed before the last line, `Results: <folder>`. With `[eval.cosine]`, once every pair of a repeat is done, each contestant's non-empty text answer is compared with every other's using a local sentence-embedding model (`fastembed`, `all-MiniLM-L6-v2`), one repeat at a time, never mixing repeats. The model's five files (`model.onnx`, about 90 MB, plus `tokenizer.json`, `config.json`, `special_tokens_map.json`, `tokenizer_config.json`) must already sit in `model_dir` (default `<config dir>/models/all-minilm-l6-v2`): fetch them by hand from `https://huggingface.co/Qdrant/all-MiniLM-L6-v2-onnx/resolve/main/`, since `fastembed`'s own downloader uses bundled roots and fails behind a TLS-intercepting proxy. A missing file is refused before anything is written, naming the file, the folder and that URL. The similarity (-1 to 1, 1 meaning near-identical wording) is saved per pair in `evals.json` and printed, two decimals, as one line per repeat after the ranking, with a `no comparable answers` line instead for a repeat whose saved cosine data has no numeric pair to show (every participant errored, was skipped, or only one answered); it's a measure of agreement between contestants, not of quality, so it's shown alongside the ranking and never changes it. A missing or empty answer is skipped, not an error; a model that fails to load, or that loads but fails while embedding, is one warning for the whole run and an `error` entry per pair instead of failing the run. With `[eval.jev]`, Jev (TypeSafe's System One model) also scores every contestant on the same rubric, after the judge and cosine, one candidate per request (see *The Jev evaluator* under *Configuration*); its results go to each pair's `evals.json` under `jev`, `run.json` records the models that answered as `jev_models`, and its ranking is printed after the judge's, never blended with it. |
| `sbxm run show <run-id> [--diff]` | Prints a saved run from its files: when it started and completed, the profile and `sbx` version, and for each contestant and repeat the status, the answer, a summary of the diff (files changed, lines added and removed) each executable check's verdict and the judge's scores, with the anonymous label each contestant was judged under. `--diff` also prints every full patch. With a judge it also prints the ranking, recomputed from the saved files (edit `run-config.toml` in the run folder to try other weights); with `[eval.cosine]` it also prints the same similarity lines `sbxm run` printed, read straight from the saved `evals.json` files without loading the model. With `[eval.jev]` it also prints Jev's ranking after the judge's (or alone, without a judge), headed with the model(s) that answered, and each pair's Jev scores with their confidence, or why Jev couldn't score it. A run that is still going or was interrupted shows what has been saved so far. Only reads files; the id must look like `2026-09-30-a1b2c3`. |
| `sbxm task init [path]` | Writes a starter `sbxm-task.toml` (the worker, reviewer, sandbox profile and gates for `sbxm task`) in the repo's root. Valid as written for a Rust repo. Refuses to overwrite. |
| `sbxm task states` | Prints the task state machine's transition table (from, event, to, action), generated from `src/task/machine.rs`'s `TABLE` so the table and the code can't drift apart (checked against `sdlc/specs/task-state-machine.md` section 5.2 by a test). Reads nothing and calls no backend. |
| `sbxm task start (--issue N... \| --workers N \| --spec FILE) [flags]` | Hands GitHub issues to worker agents. For each chosen issue it makes a task: its own sandbox (`sbxm-task-issue-<n>-<harness>`), a clone of the repo on branch `issue-<n>`, and a headless worker that follows the issue and commits. `--workers N` picks up to N open issues (`must-fix` before `should-fix`; it skips questions, issues whose `**Depends on:**` issues are still open, issues that already have a task and issues `**Related:**` to one that does); `--issue N` starts exactly the issues named (one that is blocked by an open issue is still refused). An issue whose body starts with `PR: #m` (a review finding, see `task file-findings`) belongs to that PR: when PR m is open and a branch of this repo, its task starts from the PR's branch head (fetched into the task's own repo) instead of the base branch, in a fresh sandbox, the worker's prompt says which PR it is and that the branch already has commits, and `task.json` records `continues`: the PR, its branch and the commit it started from (`base`); a closed or merged PR, or one from a fork, is refused before anything is written. The tasks run in parallel and one failing doesn't stop the others. The worker has no GitHub access: the issue text is copied into its clone. When it stops, its commits are collected into a bare repo that only sbxm and your git touch (`<base_dir>\.sbxm\tasks\<id>\repo.git`) through a verified `git bundle`: the bundle must be a plain file under 500 MB, only the task branch is fetched, and no git command ever runs inside the agent's folder. Also saved there: `issue.md`, `result.md` (what the worker wrote), the transcript and `task.json`. Flags override `sbxm-task.toml`: `--worker-harness`, `--worker-model`, `--time-limit`, `--profile`, `--base`, `--repo`. `--restart` (with `--issue`) first discards an existing task of that issue, showing what and asking like `task rm` (`--yes` skips the question), after checking that the issue is still open, the task isn't running and every other input is valid. The discard is undoable (decision 185): the old task is set aside (its sandboxes removed, its folders moved to `<base_dir>\.sbxm\tasks-replaced\<id>\`) rather than deleted outright, and deleted for good only once the new task's preparation (folder, kits and sandbox) is complete, even if the worker or its gates then fail; anything that goes wrong before that point puts the old task back and the error says so. Several issues in one `--restart` are settled the same way, each on its own: the command exits non-zero if any of them was put back, naming per issue what happened. A later `--restart` of an issue whose old task is still set aside from an unsettled earlier attempt refuses before the prompt, naming the folder and how to resolve it (move it back by hand, or delete it); no command addresses `tasks-replaced/` directly. The only other thing that can stay there is an empty remnant: when the old task was put back but its emptied folder couldn't be deleted (a folder in it held open, say), the command says so and names the folder, and the next `--restart` of that issue deletes the remnant and goes on. A remnant counts only while it holds nothing but empty folders (no file and no link at any depth, and no link on the way to it); if deleting it fails again, the refusal names it as an empty folder that is safe to delete. After the worker (unless it failed) the task's gates run (see `task gates`); then `task review` and `task finish` (or `task run` for start and review in one go). Exits non-zero if any task or its gates failed. When a gate ran and failed, the next step it prints is still `sbxm task review --issue N` (or `--spec FILE`): the review runs the gates again and gives a failure to a fix round, with the gate's own output (decision 180); `task start` itself never runs a fix round. A gate that could not run at all (the sandbox `exec` failed, the host couldn't start the command, or the host tier's clean checkout couldn't be made) is never named that way: its line says so and why, and `task review` isn't printed as the next step, since the review would have nothing to react to (issue 186). **`--spec FILE` instead starts a task from a file** (a PRD or spec, not a GitHub issue): the task id is `spec-<name>-<hash>` (the file's sanitized name plus 6 hex digits of a hash of its canonical path, so two files named alike never collide), the file is copied in as `source.md` in place of `issue.md`, and the worker's prompt points at that instead. No GitHub call is made anywhere in this path, so an omitted `--base` is read from this checkout's `origin/HEAD`; if `--repo` names a repo other than the checkout's origin (or the checkout has no `origin/HEAD`), pass `--base <branch>`. It otherwise runs the worker and gates exactly like an issue task; continue it with `sbxm task review --spec FILE`. `sbxm task finish --spec FILE` then delivers it through `[finish] sink` (see `task finish`); `--restart`, `--workers` and continuing an open PR don't apply to it. |
| `sbxm task review (--issue N \| --pr N \| --spec FILE) [--repo owner/name] [--base b] [--reviewer-harness h] [--reviewer-model m] [--reviewer-time-limit 45m] [--time-limit 2h] [--profile p] [--force]` | Has a task's change reviewed by an independent agent, then lets the worker fix what it found, as many times as `[worker] fix_rounds` in `sbxm-task.toml` allows (default 3). Checks first, before anything is created: the task is ready for review, and the reviewer's provider secret (and the profile's) are stored. The gates run first unless they passed since the last change (a failure feeds a fix round too, counted against the same budget, with the gate's own output in the fix prompt instead of a review; once the budget is used with a gate still failing, the review stops there, exit 1). In an issue or spec review, a gate that could not run at all (the sandbox `exec` failed, the host couldn't start the command, or the host tier's clean checkout couldn't be made) never gets a fix round, whatever budget is left: the review stops there at once, with a message saying it could not run and why, naming `gates.log` (issue 186). `--pr` has no fix rounds, and reports such a gate as it reports any failed gate. The reviewer runs in its own sandbox over its own clone of the task branch (made from the host-owned repo, so it sees exactly the collected commits and can't touch the worker's folder), told to follow the repo's `sdlc-code-review` skill, to change nothing and to write `.sbxm-task/review.md` whose first line is `Must-fix findings: <count>` and whose next line (one blank line between the two is fine, but nothing else — a `Risk:`-looking line further down is never read as this one) is a risk assessment, `Risk: low|medium|high` plus a `## Risk` section of reasons (decision 176): low is docs, tests or a contained change with tests; medium is a behavior change in one area, or a failed/unresolved finding; high is security-sensitive or cross-cutting code, a change with no test, or must-fix findings left. sbxm raises that level to a floor from path rules (never lowers it): language-neutral defaults (CI/workflow files, dependency manifests and lockfiles, files named like secrets) plus `[risk]` in `sbxm-task.toml`; a missing or unparseable `Risk:` line is recorded as `unknown` (never silently `low`) with a warning. The level and reasons are recorded in `task.json` and shown at the top of the PR description `task finish` opens (see below) and, for `--pr N` below, the posted review comment; this is information only and changes nothing else sbxm does. Codex runs with high reasoning effort. The review is saved under the reviewer's name (`Reviewer: codex (model)`) as `review-<round>.md` and `review.md`; a review without that first line, or whose count differs from the `### <id> - <title>` findings it lists under `## Must fix`, is kept for reading but not used (the task fails at that round; run `task review` again). The first review (and any after the budget is spent and must-fix findings remain) covers every commit on the branch; if it counts must-fix findings and a round is still in the budget, the worker gets a fix round in its own sandbox (the review and a fix prompt are put in its folder; its new commits are collected like the first time), the gates run again, and the next review is scoped only to the commits made since the last one. Once a review like that finds nothing, one more review of every commit runs to confirm before the task is `ready`. From round 2 on, the reviewer sees every earlier round's review, not only the one right before (so a narrow round that misses nothing new can't hide an older finding that is still open), and is asked to mark each finding `Repeat of: <id>` (naming the earlier finding it repeats, in the same file — either side may list more than one, line numbers aside) or `new`; a must-fix finding that validly repeats one stops the task at once (`stopped` = `repeat-finding`), even with rounds left in the budget, because fixing it clearly isn't working — a claim naming a different file, or an id no earlier round has a must-fix finding for, counts as new instead and never stops it by itself, with a warning so the wrong claim isn't silently dropped. The reviewer's sandbox and clone are removed after each round, also after an error. The task ends `ready`; must-fix findings left once the round budget runs out are reported, not an error, and the task's `stopped` field is set to `rounds-exhausted` (or `repeat-finding`, above); `task status` shows it, and `task resume --rounds N` gives it more fix rounds. A reviewer like the worker's harness warns (less independent). For an issue's task nothing is pushed or posted. **`--pr N` instead reviews an open pull request from a branch of this repo** (a fork's PR is refused, as is a closed or merged one, before anything is created): the PR's head is fetched into the host-owned repo (with git's object check on), the PR's description and the issues it closes are the reviewer's context, and the gates run on a clean checkout: the sandbox tier inside the reviewer's own sandbox (a PR has no worker), the host tier as usual. The reviewer runs, with no fix round (a PR task's `fix_rounds` is always 0), and a valid review is posted as a comment on the PR (`@mentions` are neutralised and a very long review is cut with a note; the whole text is `review.md` in the task folder). Nothing is posted if the gates fail or the review is invalid; if only the posting fails, the review stays saved and the error gives the `gh pr comment` command to post it by hand. A PR task that exists is retried when its review failed, as before; **a `ready` task is reviewed again instead of refused, once its head has changed** (decision 190, gap G34): `--pr N` fetches the PR's current head and, when it is still the commit the task's last review covered, refuses before anything is written, with `already reviewed at <commit>` and a hint naming `--force`; `--force` reviews that head again anyway. Once it proceeds (a changed head, or `--force`), the gates run again (the commits are new), the earlier round's `review-<n>.md` stays in the task folder (never overwritten), the reviewer gets every earlier round as context the same way a re-reviewed issue task's reviewer does, and `review.md` opens with a "Status of earlier findings" section naming every earlier finding (must-fix, should-fix and question alike) fixed (with the commit), still open, or deferred to a GitHub issue — `sbxm task rm --pr N` is no longer needed first. **A PR whose head the in-task review already covered isn't reviewed again** (decision 187): when an issue task on this machine opened PR N with `task finish` and the review recorded in its `task.json` covered the commit that is the PR's head now, `--pr N` refuses before anything is written, with `already reviewed at <commit>` and a hint naming `--force`, which reviews it as before. Any later commit on the PR (a hand fix, a rebase) is reviewed without `--force`. Only the local task folders decide, compared with the PR's head: a comment on GitHub that looks like a review changes nothing, and with no task folder recording the PR (after `task rm --issue N`, or on another machine) the PR is reviewed as before. A task that continued an open PR doesn't count, since its review covered only its own commits. `--repo` and `--base` apply to PRs (defaults: this checkout's origin, the repo's default branch). **`--spec FILE` instead continues a spec task** (started with `task start --spec FILE`): the same gates, review and fix-round loop as an issue's, with `review.md` its only output (no PR to comment on, no GitHub call made); the printed "ready" line suggests `sbxm task finish --spec FILE` as the next step, which delivers it through `[finish] sink` (see `task finish`). |
| `sbxm task resume (--issue N \| --spec FILE) [--rounds N] [--forget-ambiguous]` | Continues an issue or spec task from the stage `task.json` recorded, keeping its clone, its commits and its folder, and carries it on to `ready` the way `task review` does (gates, review, fix rounds), printing the same lines. Refused while the `sbxm` process that recorded the task is still running (the pid with the same start time, as `interrupted` is decided), naming that pid, and for a PR task (no worker or fix rounds to resume; review it again directly with `task review --pr N`, or, from a state that command refuses, such as a failed preparation, remove the task first with `task rm --pr N`). What it does depends on where the task stopped: a **preparation** that failed or was cut off makes the worker's sandbox afresh over the kept clone (removing a half-made one first), then runs the worker; a **worker** that failed or was cut off runs again in the same clone, which keeps what the last run committed; **gates** cut off run again (never recorded as `gates-failed`); a **review** that failed or was cut off runs the reviewer again (its gates passed for the same commits); a **fix round** that failed or was cut off runs again with the prompt and the input it was given (the review, or the failed gate's output, kept in the task folder as `gate-failure.md`), and is counted once; a review that **completed** but was never acted on is replayed from the result `task.json` recorded when it was parsed (`review`: round, must-fix count, scope, repeat), so the reviewer doesn't run again and `review.md` isn't read back (an older record without it runs the reviewer again); a worker or fix round that **ended** but was never reviewed has its committed work collected again (in case collecting was cut off), then is reviewed. The worker's sandbox is made again first whenever it is gone, since the gates and the fix rounds run there too, from the harness, model, profile, CPUs and memory `task.json` recorded when the task started, never from what `sbxm-task.toml` says now (a `--profile` flag or a later edit of the file is never silently swapped in); the CPUs and memory recorded are the values the sandbox was actually created with, not the optional overrides, so a task that inherited the global defaults keeps running under those even after the globals change. Before rebuilding it, resume hashes the recorded profile's contents as they are on disk now, together with those resources and the harness, and refuses — before touching the sandbox or writing anything — if the result no longer matches the task's `config_hash` (the profile was edited since the task started), naming `task start --restart --issue <n>` for an issue task or `task rm --spec FILE` for a spec task, or restoring the profile, as the fix; a record saved before this was added has no profile or resources to rebuild from and resume refuses, naming `task rm` to start the task again. The reviewer, the gates and the time limits are always reloaded fresh from `sbxm-task.toml`. **A task that stopped `ready`** (`stopped` in `task.json`) gets another fix round from its `review.md`: `--rounds N` (N of 1 or more) adds N to its `fix_rounds` and is needed once the rounds are used (`rounds-exhausted`); a `repeat-finding` stop with rounds left resumes without it. `stopped` is cleared (a later stop sets it again), and the first review after that fix round doesn't stop the task on a repeated finding, so it isn't stopped again at once. A `ready` task that didn't stop, a finished one, and `--rounds` on a task that isn't `ready` are refused, each naming what to run instead. The worker is the one the task recorded; the reviewer, the gates and the time limits come from `sbxm-task.toml`. A cloud task whose record holds an **ambiguous create** (one sbxm started and never heard return, decision 268; see *Running tasks in Docker Cloud Sandboxes*) is refused before anything is touched; remove or keep that sandbox by hand, then `--forget-ambiguous` drops it from `task.json`, with no cloud call for it, and resumes. Exit 1 when the worker fails again or the gates fail. |
| `sbxm task gates --issue N [--tier sandbox\|host\|all] [--dry-run]` | Runs a task's gates, the checks `sbxm` itself runs (an agent's prompt can't skip them) to decide whether its work may go on. They are listed in `sbxm-task.toml` under `[gates]`. The **sandbox tier** (`sandbox = [...]`, default for a Rust repo: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`) runs each command as `sh -c` in the worker's own sandbox, on its folder, under a time limit enforced inside the sandbox (`timeout = "20m"` per command); the first failure stops the tier. The optional **host tier** (`host = [...]`, off while empty) runs on this machine, in a clean checkout of the task branch from the host-owned repo (only committed files, no hooks), only after the sandbox tier passed, and the checkout is removed afterwards. **Host gates run agent-written code on your machine, outside any sandbox**: list only commands you would run on a stranger's pull request. Results go to the task's `task.json` and `gates.log`; a failure marks the task `gates-failed`. `task start` runs them after the worker; this command runs them again on demand, for example after you fixed something by hand in the worker's clone: first it collects what is *committed* there into the task's repo, so the host tier, `task review` and `task finish` use the same commits as the sandbox tier (commit your fix; uncommitted changes are seen by the sandbox tier but are never collected). `--dry-run` prints each tier's commands, where they run and whether the tier is off, and changes nothing. A run of one tier is partial: `task review` skips its own gate run only if the passing runs together covered every configured tier on the task branch's current commit, and `--tier host` is refused unless the sandbox tier passed on that commit. Refused while the worker is running or after the task has moved past its gates. |
| `sbxm task status [--issue N \| --pr N] [--json]` | One line per task: stage, status (`interrupted` when its `sbxm` process is gone), `ahead:` the commits on the task branch past its base in the task's own repo (`-` before the repo exists; for a task that continues a PR, past the commit the PR's branch was at when the task started, so the PR's earlier commits don't count), and whether `result.md` and `review.md` exist; a task that continues a PR ends with `[PR #m (<branch>)]`, and a task that stopped itself ends with `stopped: <reason>` (`rounds-exhausted` or `repeat-finding`). `--json` prints the records, plus `interrupted` and `commits_ahead` (`null` when unknown). |
| `sbxm task file-findings (--issue N \| --pr N \| --file F) [--create] [--only ID,...] [--standard-criteria] [--keep-paths] [--repo owner/name]` | Files the findings of a review as GitHub issues, one per finding. `--file` takes any review file (e.g. `sdlc/reviews/<date>-<scope>.md`); `--issue`/`--pr` take the `review.md` of that task (the built-in reviewer prompts ask for the skill's review format, with an `Issues:` line; a custom `[prompts] reviewer` must do the same). **A dry run unless `--create`**: it prints every issue it would create and the `Issues:` line it would write, and changes nothing. Before any write it checks the finding ids, that the review has exactly one `Issues:` line, your `gh` login, the repo and its labels (`must-fix`, `should-fix`, `question` must exist, and `size:S`, `size:M` or `size:L` for each size a selected finding has; none are created, and a missing one is refused like a missing severity label), and every finding: a secret-looking line (token, key, password assignment) refuses the whole run, naming the finding and line but never the value, and personal paths (`C:\Users\<name>`, `/home/<name>`) become `~` (`--keep-paths` keeps them). A must-fix or should-fix finding with no acceptance criteria is refused unless `--standard-criteria`. Each issue is labeled `must-fix`, `should-fix` or `question` by its section, plus `size:S`, `size:M` or `size:L` when the finding has a `**Size:**` line starting with S, M or L (issue 233): the line goes into the body above `Where`, and the dry run shows the label (`would create [must-fix, size:M] S-1: ...`). A finding with no size line, or another value, is filed without a size label and the report says so (`S-2 has no size (no **Size:** line), so it gets no size:* label`); it is never refused. The built-in reviewer prompts ask for that line on every finding, a question included; with `--pr N`, every body's first line is `PR: #N`. Issues are created in dependency order with permalinks at the reviewed commit and `Depends on`/`Related` links to the other issues; a hidden marker in each body means a rerun never files a finding twice (it skips it and reuses its number), and the links of issues that already exist are patched in those two fields only. A finding also skips, naming the issue that already covers it, when an open issue has the same title and PR number even without the marker (decision 169); an exact marker match, in any state, always takes precedence over that title-and-PR fallback, so continuing a PR's review doesn't duplicate what was already filed; a closed one doesn't count. Such an adopted issue, and an open issue matched by its marker, gets the finding's label, replacing another of the three (other labels stay), and the finding's size label only when it has no `size:*` label yet (a size set by hand is never changed or removed), and, with `--pr N`, its body's first line set to `PR: #N` if that line is missing or stale: the dry run prints `S-1 skipped (exists #77), would label it must-fix (removing should-fix)` and, with `--pr N`, `, would set its body's first line to PR: #7` too; `--create` reports `skipped (exists, labeled must-fix)`. **Both only happen for a selected finding** (every one by default, or the ones `--only` names, issue 98): a finding `--only` leaves out never gets a label or PR-line change on its matched issue, no matter the label or `PR:` line it already has; its `Depends on`/`Related` fields are still patched, the same as any matched issue's, selected or not, so a later `--only` batch still links up, and the dry run names that too (`would update links on #77 (S-2)`); nothing else about an unselected finding's issue changes. Afterwards the review's `Issues:` line is rewritten (`Issues: S-1 #41, S-2 pending`). Exits non-zero if some issue could not be filed; rerun to file the rest. |
| `sbxm task finish (--issue N \| --spec FILE [--push-unresolved])` | Publishes a ready task: pushes branch `issue-<n>` from the task's host-owned `repo.git` (never forced) and opens the PR from it to the task's base. **The body starts with the risk section** (decision 176): the level recorded on the last review (`unknown`, with no reasons, for an older record with none recorded) and its colored marker (🟢 low, 🟡 medium, 🔴 high, ⚪ unknown — GitHub Markdown can't color text) and reasons; a draft's own reasons (below) are added to this section too, not only to `## Unresolved`. Then `Fixes #<n>`, then `result.md` and `review.md` each under a heading (a file over 25,000 characters is cut, and the body and the output say so). All of this text but that `Fixes #<n>` line is written by an agent, so it is neutralised before it goes in: a zero-width space after an `@` that starts a word, and after a closing keyword (`close`, `fixes`, `Resolved:` and the rest, any case) that is followed by `#N`, `owner/repo#N` or a URL, so it notifies no one and closes no other issue or PR when the PR merges; the files in the task folder stay as they are. **A task with something left unresolved opens a draft PR, never a normal one:** when it stopped (`stopped` in `task.json`, `rounds-exhausted` or `repeat-finding`) or its last review has must-fix findings left (the count recorded in `task.json` when the review was read, so editing `review.md` doesn't change it; for an older record without it, the count in `review.md`, and a count that can't be read counts as left), the PR is opened with `gh pr create --draft` and its body starts with an `Unresolved` section giving each reason and listing each must-fix finding still open with its `Where:` and the review round that found it (cut at 5,000 characters, the rest counted). That list is recorded in `task.json` as each review is read, never taken from `review.md` again: a full review's findings replace it, and a narrow review's are added to the older ones it didn't see, replacing only one it names in `Repeat of:` on the same file (ids restart every round, so a claim that matches more than one keeps them all), so a stopped task's draft lists everything still open (an older record without the list rebuilds it from its saved `review-<round>.md` files, both when it is finished and before it is reviewed again, for example after `task resume`, so a narrow review doesn't drop what an earlier one left open; with no saved rounds it uses the findings in `review.md`). Every recorded finding is checked for secret-looking text before any of it is cut to fit. The output says `opened draft PR` and prints each reason. A clean task gets a normal PR. Every check comes before the push: the task must be `ready` and have no PR yet, the repo, branch and base names must be usable, the branch must have commits beyond the base, and neither `result.md`/`review.md` nor a draft's list of open findings (from `task.json`) may contain a secret-looking line (refused, naming the line, never the value). The task then becomes `finished` and `task.json` keeps the PR's URL. **The in-task review then counts as the PR's review** (decision 187): when the review recorded in `task.json` covered the commit pushed as the PR's head, `finish` posts it as a comment on the new PR: the same risk section the description opens with (from `task.json`, so the two agree), a line naming the issue and the reviewed commit, then `review.md` (mentions neutralised and cut like a `task review --pr` comment). A draft's failing review is posted the same way. The output says so; `task review --pr N` then refuses that head, and a later commit on the PR gets it as usual. If the comment can't be posted, the PR stays open and recorded, `task.json` gets a note, and the error gives the `task review --pr N` command to review the PR instead. A task whose review recorded no commit (an older record) or that has no `review.md` posts nothing. Only a post that GitHub accepted is recorded (`review.posted` in `task.json`), and only that record makes `task review --pr N` refuse: after a failed post, an interrupted `task finish` or no post at all, the PR is reviewed as usual. If the push worked but opening the PR failed, the task stays `ready` with a note and the error says so: run `finish` again, which pushes the same commits (nothing to do) and retries only the PR. If someone opened the PR by hand in between, GitHub's refusal is shown and nothing is recorded. **It refuses to push a branch that adds, edits or deletes anything under `.github/workflows/` or `.github/actions/`** (letter case ignored): GitHub would run those files with the repository's secrets the moment the branch is pushed, before anyone has read what the agent wrote, so read them in the task's `repo.git` and push the branch yourself if the change is meant. **A task that continues an open PR** (its issue starts with `PR: #m`, see `task start`; no extra flag) is published differently: its commits are pushed to PR m's own branch and no PR is opened; instead PR m gets a comment listing the commits added (short id and subject) and `Fixes #<n>` for the issue (a subject over 500 characters is cut, and commits beyond what fits in one GitHub comment are counted instead of listed; the comment says so). sbxm can't make PR m a draft, so a task that stopped or has must-fix findings left (decided as above) starts that comment with the same `Unresolved` section, each reason and each must-fix finding with its `Where:`, leaves PR m's draft status as it is, and prints each reason as `unresolved:`; a clean task's comment only lists the commits. The push is a fast-forward from the commit the task started at, never a force: it is refused, with the reason and before anything is pushed or posted, unless PR m's branch on GitHub is still at that commit (or already at the task's own commits, when a rerun only retries the comment and pushes nothing). The push itself checks that again atomically, carrying that commit as the expected old value (`--force-with-lease` pinned to it, only ever for a fast-forward), so a branch that moves between the check and the push is left as it is and the push is refused; if someone pushed to it meanwhile, or it was deleted, start the task again with `task start --restart --issue <n>`. The workflow check and the commits-needed check count only the task's own commits, past that start commit. If the push worked but the comment failed, the task stays `ready` with a note and `finish` again retries only the comment. The task then becomes `finished`, `task.json` keeps PR m's URL, and the re-review is `sbxm task review --pr m`, unchanged (a PR task already `ready` from an earlier review is reviewed again in place; decision 190). **`--spec FILE` finishes a spec task through `[finish] sink` in `sbxm-task.toml`, never a PR** (so it posts no review either; nothing of decision 187 applies to it): with `local` (the default) the branch stays in the task's host-owned `repo.git`, nothing is pushed, the task becomes `finished`, and the output gives the `git fetch <repo.git> <branch>:<branch>` command that copies it into your checkout. The task must be `ready` with commits beyond its base. With `sink = "push"` it pushes the branch to `origin` instead and opens no PR; the output says `git fetch origin <branch>`. The push only ever creates the branch (it carries an empty `--force-with-lease`, so it is never forced): a branch of that name already on `origin` at another commit is refused and left alone, and one already at the task's own commit (an earlier push whose record was lost) is only recorded. A task that stopped (`stopped` in `task.json`) or whose `review.md` has must-fix findings left is refused unless `--push-unresolved`, since a spec task has no PR or draft to mark it unfinished, and a branch that changes `.github/workflows/` or `.github/actions/` is refused as for an issue task. |
| `sbxm task rm (--issue N \| --pr N \| --spec FILE \| --merged) [--yes] [--force] [--dry-run] [--forget-ambiguous]` | Deletes a task: its sandboxes (the worker's and the reviewer's, by the names saved in `task.json`), its clones (`<base_dir>\tasks\<id>`, `<id>-review`, `<id>-gates`) and its folder `<base_dir>\.sbxm\tasks\<id>\`. It first prints the exact sandboxes and paths and asks `[y/N]`; without a terminal it refuses unless you pass `--yes`. It refuses while the task is running (an interrupted one can go), and builds paths only from an id that looks like `issue-41`, `pr-7` or `spec-idea-1a2b3c`. **A spec task's result is protected:** under the `local` sink its branch exists only in the task's `repo.git`, which `rm` deletes, so `rm --spec` refuses (before asking anything) while the branch has commits beyond its base that the checkout you run it from doesn't have, and names the `git fetch` command; fetch it first, or pass `--force` to delete it anyway. It never follows a link or junction: a task folder that is one, or that resolves anywhere but its own place under the base folder, stays (a link *inside* a clone is removed, not followed). Removal carries on after a failure and says what went and what stayed; the task folder is deleted last and only if everything else went, so a second `rm` still knows the sandbox names. Exits non-zero if anything stayed. A cloud task whose record holds an **ambiguous create** (decision 268) is refused before anything is removed, naming the sandbox and the `sbx --cloud` commands to look and remove it by hand; `--forget-ambiguous` drops it from `task.json` without any cloud call for it, then removes the rest (with `--merged`, a task like that is otherwise left alone, with the reason). **`--merged` instead removes every task whose work is merged** (decision 195, issue 202), in one command instead of a `task rm` typed by hand for each: an issue's task whose PR is merged on GitHub, a PR-review task (`pr-N`) whose PR is merged, and a spec task whose branch is reachable from its base branch as `origin/<base>` stands after a fetch (a squash or rebase merge isn't detected, so a task like that is left alone and listed as "not merged", naming the plain `task rm --spec` as the way to remove it by hand). A task whose PR is open, or closed without merging, is left alone without comment; a task whose result exists only in its own `repo.git` and was never fetched (decision 178's protection) is left alone too, listed with the reason, even though it counts as merged. A task whose folder is a link or junction (or resolves anywhere but its own place) is listed as left alone before anything is read through it, and so is one whose `task.json` is a link or names another task than its folder, or a spec task whose `repo.git` is a link (no `git fetch` runs through it); if `.sbxm\tasks` itself is a link, the whole command refuses. No archive is ever written through a linked `.sbxm` or `task-archive` folder. `--force` is refused together with `--merged` (there is nothing of its own to force). For a spec task with the `local` sink it never deletes the worktree or the local branch in your own checkout either; it names the two `git` commands that do, without running either. Before removing anything it copies each task's text records — `review.md`, every `review-<n>.md`, `result.md`, `run.log`, `gates.log` and `task.json`, whichever exist, never transcripts, clones or `repo.git` — to `<base_dir>\.sbxm\task-archive\<id>\`, and names that folder in its output; nothing ever deletes or overwrites an archive. A rerun after a removal that left something behind (a sandbox `sbx` could not remove) reuses the archive it made the first time when it still holds exactly the same records, so running `task rm --merged` again finishes the job; an archive that differs is refused, naming the folder to move aside. `--dry-run` only makes sense with `--merged`: it lists every task it would remove and why, and every task left alone and why, and removes nothing (no question asked either). Without `--dry-run`, every merged task is listed once and confirmed together (`[y/N]`, or `--yes` without a terminal), then archived and removed, reporting the disk given back on the base dir's drive, and on the drive holding `sbx`'s sandbox state too (`sandbox_state_dir` when configured, otherwise the platform default, `%LOCALAPPDATA%\DockerSandboxes` on Windows) when it is on a different one, each when it can be read; the two are printed as one line when they share a drive. |
| `sbxm task logs (--issue N \| --pr N \| --spec FILE) [--follow]` | Prints the task's `run.log` (decision 170). With `--follow`, keeps printing new lines as the task's own command writes them, until you press Ctrl+C; it doesn't stop by itself when that command ends. Read-only: it never writes to the log, not even its own header. An unknown task (no `task.json` in its folder) gets the `sbxm task status` hint; a task with no `run.log` yet says so, naming the folder, and exits 1 (`--follow` doesn't wait for one to appear); a `run.log` that is there but can't be opened or read (a permission error, a lock, a folder) gets `cannot open <path>: <os error>` instead, also exit 1. |
| `sbxm task run (--issue N \| --spec FILE)... [flags]` | `task start` and then `task review` for one task, with the flags of both (`--worker-harness`, `--worker-model`, `--reviewer-harness`, `--reviewer-model`, `--time-limit`, `--reviewer-time-limit`, `--profile`, `--base`, `--repo`, `--restart`, `--yes`). The reviewer's flags are checked before the worker starts. It stops at `ready` and prints the `task finish` command: publishing stays a separate step. Gates that ran and failed right after the worker don't stop it: it goes on to the review, which runs them again and gives a failure to a fix round (decision 180). A gate command that could not be executed at all — the sandbox or the host couldn't even start it, or its clean checkout couldn't be made — stops it after the start phase instead: the review would have nothing to react to, so it isn't named as the next step (issue 186). A failed worker stops it before the review, and so does a gate run that could not record its results. Exit 1 for any failure, including gates still failing once the review's round budget is used; a task that ends `ready` with must-fix findings left is reported, not an error. **`--spec FILE` instead carries through a spec task** (decision 174(d), issue 194): `task start --spec` then `task review --spec`, with no GitHub call anywhere in the path; it ends with the `task finish --spec` line, same as the three commands typed by hand. `--restart` discards an existing spec task the same undoable way `--issue` does (decision 185), except a finished (or merely worked) result that lives only in the task's `repo.git`, not yet fetched into your checkout, is protected the same way `task rm --spec` protects it (decision 178): fetch it first, or run `task rm --spec FILE --force` before restarting, since `--restart` has no `--force` of its own. **`--issue`/`--spec` are repeatable and may be combined into a list** (decision 193, issue 201), run one after another, in the order written on the command line (e.g. `--issue 210 --issue 202 --spec sdlc/specs/x.md`); the other flags above apply to every task of the list, and there is no list file. Every target's own selection problem (an issue that's a question, blocked by an open `Depends on` issue, related to another issue in the same batch or an existing task, doesn't exist, isn't open, already has a task, or names a PR that can't be continued; a spec file's readability; either naming a task that already exists) is checked before the first target's sandbox is created, so a later target's problem surfaces up front instead of only once an earlier, valid target has already run (issue 201 review M-1); skipped, not an error, when the list's shared repo can't even be resolved, in which case each target is checked the old way, on its own turn. The tasks are independent: a later one never gets an earlier one's work. A task that stops short of `ready` is recorded and the list goes on to the next one; the list itself stops only for a problem that would hit every task: `sbx` not answering (whether the preflight's own call to it, or `SandboxBackend::exec` itself failing while a worker, fix round or reviewer ran — a harness-reported failure or an ordinary failing gate doesn't count, since `sbx` ran fine there), a missing secret, or too little free disk (the free-disk check — issue 210 — runs again before each task, not only the first, since a task's own sandbox can fill the drive the next one needs). It prints a summary line per task at the end: its stage, its must-fix findings left, and the next command. Exit 0 only when every task of the list ended `ready`; 1 when at least one failed or was never started because the list stopped. `--restart` is refused when the list has more than one task: there is nothing left to show the user when several tasks' old copies would be discarded at once. |
| `sbxm config show [project] [--profile p] [--harness h] [--kits]` | Prints the merged config exactly as it's hashed, the hash, and with `--kits` the generated kits. For a project whose sandbox came from a template, the template variant (its `[template].allow`, no install steps); with `--profile`, the install variant. Creates nothing. |
| `sbxm config profiles-dir` | Prints the folder profiles are read from (`profiles_dir`, else `<config dir>/profiles`), for scripts such as `just deploy-profiles`. Loads only `config.toml`, so it fails like any command on an invalid or missing one. |
| `sbxm template build <profile> [--harness h] [--warm <repo>] [--ignore-disk]` | Builds the profile's template for one harness (`claude` by default; `codex` and `pi` too, others are refused): creates a build sandbox `sbxm-template-<profile>-<harness>` with the profile's install steps and its `[network]` allow list, runs the warm-up when `--warm` is given, stops it, saves it with `sbx template save` under a fresh tag (`sbxm-<profile>-<harness>:<hash12>-<UTC time>`), and removes the build sandbox and its workspace, also after a failure. Prints the tag and writes the record `<base_dir>\.sbxm\templates\<profile>-<harness>.json`. The previous template of the same profile and harness is then removed, unless a sandbox or task still records it (the build says so and it stays until a later build). Refused before anything is written for a profile without `[template]`, an `sbx` older than 0.46.0, a drive under `min_free_disk`, or a second build of the same profile and harness while the first is still running; leftovers of an interrupted build are removed first. `--warm <repo>` copies the repo in like a seed and runs its `[warm] commands` from its `sbxm-task.toml` (with its `lock_files`), or for a repo with a `Cargo.toml` and no `[warm]`, `cargo fetch`, `cargo clippy --all-targets` and `cargo test --no-run` (all `--locked`) with `Cargo.lock` as the lock file. Each warm-up step is printed on stderr before it runs; a step's own output is shown only when it fails. A failing step saves nothing. Only caches outside the repo (`~`, `/tmp`) end up in the template. |
| `sbxm template ls` | One line per template record: profile, harness, tag, cold or warm, size, age, and how many sandboxes and tasks use it; flags a record whose image is gone from `sbx`. |
| `sbxm template rm <profile> [--harness h]` | Removes the template's images (its tag and any earlier tags kept for a user) and its record. Refused while any sandbox (`state.json`) or task (`task.json`) records one of them, naming each; remove those first. Refused while a build of the same profile and harness is running. |
| `sbxm doctor` | Checks `sbx` (on `PATH`, new enough, daemon answering), the config, every profile, every project with each of its sandboxes (secrets stored, kits valid), the base dir (exists, writable, not a temp folder, at least 10 GiB free), the free space of the drive `sbx` keeps its sandboxes on (see *The sandbox drive*), and the templates: one line each with size and age, the total (the count and summed `sbx` sizes of every recorded image `sbx` still lists, previous ones a rebuild kept because they are in use included; only the recorded sizes of the current images when `sbx template ls` fails), and a `warn` line for an image gone from `sbx`, a stale template (the profile's install steps or build allow list changed), a profile that no longer has `[template]`, or a template over 14 days old (its agent is frozen at the version it was saved with). An `sbx` older than 0.46.0 fails only when a profile has `[template]`. Each line starts with `ok`, `skip` (a check that couldn't run, with how to make it run), `warn` (a problem that doesn't fail `doctor`) or `FAIL`. Exits non-zero if anything fails. |
| `sbxm status [--watch] [--interval secs]` | The Tasks dashboard (decision 171): one row per task — id, stage, status (`interrupted` when its `sbxm` process is gone, decided the same way `task status` decides it), time in the current stage (the last stage stamp in `task.json`; `45s`, `12m`, `1h05m`, `3d02h`), commits ahead of base, must-fix findings left (from the last review `task.json` recorded), the PR link in full, and the last `run.log` line that isn't a header, without its timestamp. A missing value shows `-`. On a terminal the last column (the log line) is cut to the terminal's width, counted in display columns (a wide character such as `界` or `😀` takes two), with `…`; on a terminal too narrow for the other columns their spacing shrinks first (to one space, then none), and a nonempty log line always keeps at least its `…`. A pipe or a file never cuts it. `--watch` redraws every `--interval` seconds (whole seconds, minimum 1; an error without `--watch`; default 2) until Ctrl+C, clearing the screen each time; on a pipe or a file it always prints once and exits, `--watch` or not. Reads only `task.json`, each task's `repo.git` (for the commits ahead) and `run.log`; never calls `sbx` (a missing sandbox is the Sandboxes section, a separate issue). Rows are in the same order `sbxm task status` lists them. |

`--harness` defaults to `claude` for `new` and `config show`; `open`, `stop` and `rm` (without `--purge`) require it, so they never act on the wrong sandbox by default (issue #33). `sbxm <command> --help` shows every option.

Every command that can create a sandbox (`new`, `open` when it creates or rebuilds one, `run`, `template build`, and
`task start`, `task review`, `task run` and `task resume`) first checks the free space of the drive `sbx` keeps its sandboxes on and
refuses, before writing anything, when it is under `min_free_disk`, naming the drive, the free space and the flag
that overrides it: `--ignore-disk` lets that one command through. See *The sandbox drive*. A task whose sandboxes
run in the cloud skips this check, since they don't use that drive (`--ignore-disk` is still accepted); see
*Running tasks in Docker Cloud Sandboxes*.

### The `justfile`

`just` lists the recipes. Recipes run sbxm through `cargo run`, so they always use the current code, and the
`justfile` uses PowerShell 7 (`pwsh`) as its shell on every platform.

**Setting up**

| Recipe | What it does |
|---|---|
| `just deploy-profiles` | Copies the repo's `profiles/*` into the `profiles_dir` sbxm reads, asking `sbxm config profiles-dir` (so an invalid or missing `config.toml` stops it before anything is written). Each repo profile replaces its copy there; profiles that exist only in `profiles_dir` are left alone. It prints `new`, `updated` or `unchanged` per profile. Run it after `git pull` changes a profile; sandboxes built from the old one show config drift until you rebuild them. |
| `just init` | Writes a starter config and `default` profile (refuses to overwrite). |
| `just doctor` | Checks `sbx`, the config, every profile and project, and the base dir. |
| `just install` | Installs `sbxm` on your PATH from this checkout. |

**Using sbxm** (harness defaults to `claude`, as in the recipes' arguments)

| Recipe | What it does |
|---|---|
| `just new <project> [harness] [profile]` | Creates a project and its sandbox, e.g. `just new demo codex`. |
| `just seed <project> <dir> [harness]` | Creates a project from a seed folder. |
| `just open <project> [harness]` | Attaches to the sandbox, creating it if needed. |
| `just rebuild <project> [harness]` | Recreates the sandbox from the current config (session history is lost; the workspace is kept). |
| `just stop <project> [harness]` | Stops the sandbox. |
| `just rm <project> [harness]` | Removes the sandbox and state; the workspace is kept. |
| `just purge <project>` | Removes every sandbox of the project and deletes its workspace (asks first). |
| `just list` | Lists sandboxes with status, config drift and orphans. |
| `just show [project] [harness]` | Prints the merged config, its hash and the kits, without creating anything. |
| `just demo [project]` | Creates one project with a sandbox for every harness, then lists them. |

**Developing sbxm**

| Recipe | What it does |
|---|---|
| `just build` | Builds the debug binary. |
| `just test` | Runs the Rust tests (no Docker needed). |
| `just script-test` | Runs the Pester tests for `scripts/deploy-profiles.ps1` (needs Pester 5). |
| `just check` | Everything that must pass before a commit: `cargo fmt --check`, clippy, `cargo test` and `just script-test`. |
| `just real-test [base_dir]` | Runs the tests against the real `sbx` (needs `sbx login` and a base dir not on `C:`). |

## Projects, sandboxes and where things live

- **Project names** use lowercase letters, digits and `-`, at most 40 characters, and can't be `default` or a
  reserved Windows name such as `con` or `com1`.
- **Workspace:** `<base_dir>/<project>`, shared by all of the project's sandboxes.
- **Sandboxes** are named `sbxm-<project>-<harness>`, so a project can have one per harness side by side.
- **sbxm's metadata** lives in `<base_dir>/.sbxm/<project>/`: `state.json` (which sandbox was built from which
  profile and config hash), the generated kits, and your optional `sandbox.toml`. This folder is never mounted, so an
  agent can't read or change its own config.

## Harnesses

| `--harness` | Agent | Mandatory instructions file | Notes |
|---|---|---|---|
| `claude` (default) | Claude Code | `~/.claude/CLAUDE.md` | Supports `harness.claude.home_files` and `harness.claude.managed_settings`. |
| `codex` | Codex | `~/.codex/AGENTS.md` | |
| `gemini` | Gemini CLI | `~/.gemini/GEMINI.md` | `sbx`'s skills store doesn't serve Gemini: sbxm warns unless `skills.store = "off"`. |
| `antigravity` | Antigravity (`agy`) | `~/.gemini/AGENTS.md` | Uses the Antigravity kit from Docker Hub, pinned to a fixed tag. With a `google` secret stored (`sbx secret set -g google`), `agy` uses it as a Gemini API key and needs no sign-in (decision 137). Before every headless run (`sbxm run`, `sbxm task`), sbxm removes `sbx`'s `mcp-gateway` entry from `~/.gemini/config/mcp_config.json`, since `agy` waits on that server and hangs until the timeout; other MCP servers stay (issue 287). A headless run with no model set (an `sbxm task` worker or reviewer without `--worker-model`/`--reviewer-model` or a `model` in `sbxm-task.toml`, or an `sbxm run` contestant or judge whose run-config entry leaves out `model`) passes `--model gemini-3.8-flash-medium`, so `agy` never picks its own; `task.json`, the saved review, `run.json`, `result.json`, `judge.json`, `evals.json` and the output of `sbxm task`, `sbxm run` and `run show` name the model that ran, for example `antigravity (gemini-3.8-flash-medium)` (issue 314). Claude and Codex have no default: their run-config entries still need `model`. Interactive `new`/`open` are unchanged: switch there with `/model`. `sbx`'s skills store isn't known to serve it: sbxm warns unless `skills.store = "off"`. |
| `pi` | Pi | `~/.pi/agent/AGENTS.md` | Uses the Pi kit from Docker Hub, pinned to a fixed tag. See below. `sbx`'s skills store doesn't serve Pi either: sbxm warns unless `skills.store = "off"`. |

Settings a harness can't use are never dropped silently: `sbxm new`/`open` print a `warning:` line for each, e.g.
`harness.claude.managed_settings` on a Codex sandbox.

**Pi, first time:** the Pi kit asks `sbx` for your `anthropic` credential, and `sbx` wants your approval once. Run
your first `sbxm new … --harness pi` in a terminal and approve when asked (`This kit wants to use these credentials:
… [A]pprove all · [R]eview each · [N]o`). `sbx` saves the approval (on Windows in `%APPDATA%\sbx\credentials.yaml`).
Without it, the sandbox starts but every model call fails with `401`.

**Pi and reference instructions:** Pi reads instruction files from every parent folder, so `instructions.reference`
is always in Pi's context instead of on demand; sbxm warns about it.

## Configuration

Three layers, merged in order: the global `config.toml`, a named **profile**, and an optional per-project
`sandbox.toml`. Unknown keys are errors in all three, so a typo never silently falls back to a default.

### `config.toml`

```toml
base_dir = 'E:\sbxm-projects'                          # where projects live
profiles_dir = 'C:\Users\you\.config\sbxm\profiles'    # default: <config dir>/profiles; keep it in git
default_profile = "default"                            # used when --profile isn't given
min_sbx_version = "0.43.0"                             # checked by `sbxm doctor`
min_free_disk = "10g"                                  # default; "0" switches the sandbox-drive check off
# sandbox_state_dir = 'C:\Users\you\AppData\Local\DockerSandboxes'  # where sbx keeps its sandboxes

[resources]            # per sandbox; sbx would otherwise take all CPUs and 16 GiB
cpus = 4
memory = "8g"
```

There's no `default_harness` setting: use `--harness`.

#### The sandbox drive

Each sandbox's disk lives in the folder `sbx` keeps its state in, not in `base_dir`, and it never shrinks: deleting
files inside a sandbox frees nothing on the host until the sandbox is removed. A full drive makes `sbx` answer
`try: sbx login`, which looks like a login problem. So `sbxm doctor` reports that drive's free space, and the commands
that create a sandbox refuse when it is low:

- **Threshold:** `min_free_disk`, a size with a unit (`k`, `m`, `g` or `t`, powers of 1024: `"10g"` is 10 GiB),
  10 GiB when unset. `"0"` switches the check off, in `doctor` and in every command. `--ignore-disk` skips it for one
  command.
- **Which folder:** `sandbox_state_dir` if set, on any platform. Otherwise, on Windows, `%LOCALAPPDATA%\DockerSandboxes`
  (the only location verified; `sbx` 0.46.0 can't move it and doesn't report it). On macOS and Linux sbxm doesn't
  guess: without the key the check is skipped, and `doctor` says so in a `skip` line naming the key.
- **When it can't be checked:** if the folder doesn't exist or its free space can't be read, `doctor` warns and the
  other commands go ahead.
- `doctor` only warns about a low drive (a `warn` line, not a failure): the commands refuse on it themselves.

### Profiles: `<profiles_dir>/<name>/profile.toml`

Pick one with `--profile <name>` (1–40 lowercase letters, digits and `-`, starting with a letter or digit). Every key
is optional.

```toml
description = "Strict profile"          # for you; not part of the config hash

[network]                               # egress is deny-by-default
allow = ["github.com", "*.githubusercontent.com", "registry.npmjs.org"]
deny = []

[env]                                   # set inside the sandbox
RUST_LOG = "info"

[secrets]                               # must be stored in `sbx` (global service secrets)
services = ["anthropic", "github"]

[instructions]                          # paths relative to this profile's folder
mandatory = "instructions/mandatory.md" # always loaded, in each harness's own file (table above)
reference = "instructions/reference.md" # background material the agent can look up

[[setup.install]]                       # run once when the sandbox is created
command = "apt-get install -y ripgrep"
user = "0"                              # root when unset
description = "install ripgrep"

[skills]
store = "readonly"                      # sbx's shared skills store: "readonly" (default) or "off"

[harness.claude]                        # Claude Code only
home_files = "claude-home"              # folder copied into the sandbox's home directory
managed_settings = "managed-settings.json"
```

Rules sbxm checks when a profile loads (before anything is created):

- Env names are shell identifiers and can't start with `SBXM_` (sbxm sets `SBXM_CONFIG_HASH` and `SBXM_PROFILE` in
  every sandbox). Env values can't contain `${{`.
- Every file or folder path must be relative and stay inside the profile's folder, and must exist. No symlink or
  junction may sit on the way to it, whether the file itself or a folder above it: `instructions.mandatory`,
  `instructions.reference`, `home_files` and `managed_settings` are all checked, and the same applies when these
  keys are set in a project's `sandbox.toml`.
- `skills.store = "readwrite"` is refused: an agent could plant skills that every other sandbox then loads.
- `home_files` must name a subfolder of the profile's (or project's) folder; it can't resolve to that folder
  itself (e.g. `"."` or `""`).
- `home_files` may not contain links anywhere in its tree (in addition to the check above on the way to the
  folder itself), `.claude/settings.json` (the Claude kit replaces it; use `managed_settings`), or
  `.claude/CLAUDE.md` while `instructions.mandatory` is set.
- `managed_settings` must hold a JSON object in Claude Code's settings format, e.g. hooks or permission rules. It's
  written to `/etc/claude-code/managed-settings.json`, which takes precedence over settings the agent can edit. It's
  a strong default, not a lock: the agent has passwordless `sudo` in the Claude image.

### Per-project overrides: `<base_dir>/.sbxm/<project>/sandbox.toml`

Same keys as a profile; paths are relative to that folder. Merged over the profile:

- lists are **appended** (`network.allow`, `network.deny`, `secrets.services`, `setup.install`),
- maps are merged per key, the project winning (`env`),
- single values replace the profile's when set (`skills.store`, instruction files, `home_files`,
  `managed_settings`).

A project can add to what its profile allows, never remove from it.

### Templates: `[template]` in a profile

A profile can opt in to prebuilt templates: its sandboxes then start from a saved image that already holds what the
install steps would install, in seconds, instead of running them on every create.

```toml
[template]
strict = true      # default false
allow = []         # required: the hosts a sandbox created from the template may reach
```

- **Build it** with `sbxm template build <profile> [--harness h] [--warm <repo>]` (see *Commands*). The profile's
  `setup.install` steps and its `[network]` lists are used only while building. A sandbox created from the template
  runs none of the install steps and gets `[template].allow` instead of `network.allow` (plus what its harness's own
  kit allows). Env, instructions, home files and managed settings are applied at every create, so changing them needs
  no rebuild.
- **Every command that creates a sandbox uses it:** `new`, `open` (when it creates or rebuilds), each `sbxm task`
  sandbox (worker, reviewer, fix rounds, a resumed worker) and `sbxm run`'s contestants and judge. `state.json` and
  `task.json` record the tag a sandbox was created from.
- **When the template can't be used** (none built, its image gone, `sbx` older than 0.46.0, or stale: the profile's
  install steps, build allow list or harness kit changed since the build, or, for a warm template, the workspace's
  lock files differ from the ones it was warmed with), a **strict** profile refuses to create the sandbox, naming the
  `sbxm template build` command that fixes it; a non-strict one prints one warning and creates the sandbox with the
  install steps as before. For a task, the lock files are read at the commit its sandbox checks out, from the task's
  own `repo.git` (a PR review: the PR's head), so a PR that changes `Cargo.lock` is refused before its reviewer
  sandbox exists. `sbxm run` decides once per profile and harness, before anything is written.
- **Not in a project's `sandbox.toml`:** `[template]` there is refused, and a project that adds `setup.install`,
  `network.allow` or `network.deny` can't use the profile's template (strict: refused; otherwise the warning and the
  install steps).
- **Harnesses:** `claude`, `codex` and `pi`; a strict profile is refused with `pi`, whose own kit allows
  `registry.npmjs.org`. With `antigravity` or `gemini`, a strict profile is refused ("templates are not supported for
  this harness yet") and a non-strict one warns and uses the install steps. A strict `codex` sandbox is created with `--deny-network` for each host the built-in `codex`
  kit adds (GitHub, npm, Ubuntu, Docker) that isn't in `[template].allow`, and is removed again if `sbx` reports the
  kit allowing anything else, or if sbxm can't read its network rules. A `*.<domain>` entry in `[template].allow`
  covers every host under the domain but not the domain itself (`*.github.com` doesn't allow `github.com`).
- **Disk:** a template takes 1 to 3 GB on the drive of `sbx`'s state. A build replaces the previous one unless that
  one is in use; `rm`, `task rm` and `task rm --merged` never remove a template; `template rm` does. `doctor` lists
  them all with their total size, kept previous images included. "In use" means recorded in a project's `state.json`
  or a task's `task.json`; a running `sbxm run` isn't counted, so don't remove or rebuild a template while a
  comparison run uses it (its later repeats would fail to create).
- **Rebuild an ageing template:** a template is a snapshot of a sandbox at build time, and it can stop working as it
  ages even though nothing in the profile changed. On 2026-10-27 every Claude worker made from a 5-day-old
  `sbxm-dev` template failed to sign in ("OAuth session expired and could not be refreshed") while a plain
  `sbx create claude` sandbox signed in fine; `sbxm template build <profile> --harness <h>` fixed it. If sandboxes
  from a template fail in a way a plain sandbox doesn't, rebuild the template first (see *Troubleshooting*).

### The Jev evaluator: `[eval.jev]` in a run-config

Jev, TypeSafe's System One model, is a second evaluator next to the LLM judge. It answers the same rubric with
probabilities instead of a verdict, one contestant at a time (decision 241).

```toml
[eval.jev]
token_command = '/opt/tools/jev-token'   # an absolute path to a helper that prints the token
# token_ref = "op://vault/jev/token"     # or a secret manager reference (op://... or an ARN); exactly one of the two
# model = "jev-1.13.0"                   # default "jev-latest"; pin a version for a fixed comparison series
```

- **sbxm never reads or stores the token, and the sandbox only sees a placeholder.** Where the token itself lives (a
  file your helper reads, a secret manager) is up to your `token_command` or `token_ref`. For each repeat, sbxm asks
  `sbx` for a custom secret scoped to that repeat's evaluator sandbox (`sbx secret set-custom --sandbox
  sbxm-run-<id>-jev-<repeat> --host api.typesafe.ai --env JEV_API_KEY --placeholder sbxm-jev-<id>-<repeat> --command
  <token_command>`), creates the sandbox (a plain shell whose only egress is `api.typesafe.ai`), runs one fixed `curl`
  per call there, then removes the sandbox and the secret, even after an error. `sbx` runs your helper on the host and
  its proxy puts the token into the `Authorization` header to `api.typesafe.ai` only. The helper must sit outside every
  workspace and the temp dir (`sbx` checks it). Store the token behind `--command` or `--ref`, **never with
  `--value`**: `sbx secret ls` shows 12 characters of a `--value` secret.
- **Before anything is written,** `sbxm run` refuses an `sbx` older than 0.46.0 (custom secrets are experimental in
  `sbx`), a `token_command` that doesn't exist, and a global custom secret for `JEV_API_KEY` on `api.typesafe.ai`
  (it would make the evaluator's sandbox fail to start; the message names the `sbx secret rm` command). It needs a
  rubric.
- **What Jev sees:** the task, the contestant's final answer and its diff, never the transcript. Each request stays
  under an estimate of 28,000 tokens; a diff that doesn't fit is split per file into several requests, and a single
  file too big for one is cut (`"truncated": true`); an answer so close to the limit that a split's longer heading
  doesn't fit gets one request with the diff cut to what fits instead (also `"truncated": true`). A task or answer too big on its own gets no call and an error
  naming it.
- **What the numbers mean:** a `pass_fail` criterion is Jev's probability that it is satisfied (0 to 1, so it can sit
  between the judge's 0 and 1); a `scale` criterion is Jev's expected level mapped to 0 to 1, with its `confidence`
  recorded and shown but not used in scoring. Chunks of a split diff are averaged by size. Answers vary slightly from
  call to call; use `[run] repeat` to average them out. Each pair's `jev` entry names the model that answered
  (`jev-latest` is answered by a fixed version such as `jev-1.13.0`).
- **The judge's ranking leads.** Jev's ranking (same weights and rules) is printed after it, under its own heading,
  and never blended into one number: disagreement between them is information. Jev becomes the headline only after a
  calibration against your own verdicts.
- **When Jev fails** (token rejected, rate limited after three retries at 2, 4 and 8 seconds, an error answer, a
  timeout), that pair's entry is an `error` and a warning; the run and the judge's scores are unaffected. An error
  keeps only Jev's error type and short message, never the response body, which can echo the whole request.

## Config changes, drift and `--rebuild`

Every sandbox records a hash of what it was built from: harness, profile name, the merged profile and project
settings, the *contents* of referenced files, resources and sbxm's version. A setting that doesn't reach a harness
(e.g. Claude-only settings for a Codex sandbox) isn't part of that harness's hash.

- `sbxm list` shows `changed` when the current config no longer matches, with the command to fix it.
- `sbxm open` refuses a changed sandbox instead of attaching to something built from an old config.
- `sbxm open <project> --harness h --rebuild` recreates it. The workspace is kept, but the agent's **session history in that
  sandbox is lost**. The old sandbox is removed only after the new kits validate.
- `sbxm config show <project> --harness <h>` prints exactly what's hashed, so you can see what changed.
- A sandbox created from a template is compared with the template variant of its config (runtime settings and
  `[template]`), one created with the install steps with the install variant; building or removing a template later
  is never drift. When a non-strict sandbox was created with the install steps and its template is ready now, `open`
  prints a hint naming `open --rebuild`. If it can't tell (an unreadable template record, or `sbx` not answering), it
  warns once with the reason and attaches to the existing sandbox anyway.

## Security model

- Egress is deny-by-default and enforced by `sbx`'s proxy; only `network.allow` hosts (plus what the agent's kit
  needs) are reachable. `sbx policy log <sandbox>` shows what was allowed and blocked.
- Config only *names* secrets. Values stay in `sbx`, which injects them at the proxy.
- sbxm's metadata sits outside the mounted workspace, so an agent can't widen its own next sandbox.
- sbxm never changes `sbx`'s settings, secrets, skills store or policies. When one of them blocks something, sbxm
  tells you the command to run.
  The one secret it creates is the Jev evaluator's, scoped to its own throwaway sandbox and removed with it.
  The one policy change it makes is in a strict profile's cloud sandbox: a `policy deny` per host outside the
  runtime list, scoped to that one sandbox (see *Running tasks in Docker Cloud Sandboxes*).
- Deleting files needs `rm --purge` plus a confirmation showing the exact paths; links and junctions are refused.

## Checking on a sandbox

**Which sandboxes exist and are they running?**

| Command | Shows |
|---|---|
| `sbxm list` | sbxm's sandboxes with status, config drift and orphans (`--json` for scripts). |
| `sbx ls` | Every sandbox `sbx` knows about, with agent, `running`/`stopped` and workspace. A sandbox that has gone from this list is finished and removed. |
| `sbxm task status` | Tasks: stage and status (`interrupted` if `sbxm` died), commits ahead of base, `result.md` and `review.md`, and the recorded risk level (`unknown` with none recorded). |
| `sbxm status [--watch]` | A live dashboard over every task: the same stage/status/ahead as `task status`, plus time in the current stage, must-fix findings left, the PR link and the last `run.log` line. `--watch` redraws every 2 seconds on a terminal; a pipe or a file always gets one printout. |

Sandbox names are `sbxm-<project>-<harness>`. A task's are `sbxm-task-issue-<n>-<harness>` for its worker and
`sbxm-task-<id>-review-<harness>` for a reviewer (`<id>` is `issue-<n>` or `pr-<n>`).

**What is it doing right now?**

- `sbx exec <sandbox> bash -c 'ps aux'` runs a command inside it (this starts a stopped sandbox). Attach to the agent
  with `sbx run --name <sandbox>`.
- `sbx policy log <sandbox>` lists the hosts the sandbox reached and the ones the proxy blocked. A host under
  *Blocked requests* needs adding to `network.allow` (see *Troubleshooting*).
- Inside the sandbox, `/var/log/sbx-kit-startup.log` has the kit's startup commands. The profile's `setup.install`
  steps run when the sandbox is created and print as `✓`/`✗` lines; the `sbxm-dev` profile's take about two and a half
  minutes (mostly `just`, which is compiled).
- `sbxm config show <project> --harness <h> --kits` prints exactly what was applied, and `sbxm config profiles-dir`
  prints the folder profiles are read from.

**A task: which file changes when**

A task's files are in `<base_dir>\.sbxm\tasks\<id>\` (`<id>` is `issue-<n>` or `pr-<n>`). `sbxm task status` shows
the stage; follow a file live with `Get-Content <file> -Tail 20 -Wait`.

| File | Written while | Meaning |
|---|---|---|
| `task.json` | every stage change | The record: stage, status, gate results, sandbox names, notes. It exists from the start of `task start`, before the sandbox is ready. |
| `run.log` | every `sbxm task` command that names or starts the task | A copy of what the command printed (screen output and warnings), each line stamped with the time and the task's stage (`-` before it has one), under a header per invocation: the command line, the `sbxm` executable and its SHA-256, the commit if the build knows it, and the process id. A command that fails still gets its header and its final error, worded exactly as shown on screen, even if nothing else was printed first (`task start`/`task run --restart`'s cancellation refusal counts as this final error too). An interactive confirmation's own question (the `[y/N]` prompt) is not copied, since it is asked straight on the terminal, outside the command's normal output. It does not hold the output of the `sbx` child process (image pull and sandbox setup lines), which goes only to the terminal. Values that look like API keys or tokens (`sk-…`, `ghp_…`, `github_pat_…`, `AIza…`) are masked in it. `task rm` deletes it with the task. A `run.log` an earlier invocation already started still gets a fresh header on this invocation's first write to it, so a second invocation's lines are never left trailing after someone else's header. Writing to it is best-effort: if `run.log` cannot be appended to (for example, the folder became read-only), the command still runs and prints normally, but prints one `warning: cannot write <path>: <cause>` line to the screen the first time that happens, not once per line. If the log cannot be opened in the first place (a broken global config, an unreadable `sbxm` executable), the command still runs and prints its own error normally, but also prints one `warning: cannot open the task log …` line naming the cause, so the missing provenance isn't silently dropped. A command with no task to log into (`task init`, `file-findings --file`) stays silent; there is nothing to warn about. |
| `gates.log` | the gates run | Each gate command with its exit code and the end of its output. A failure here marks the task `gates-failed` and stops it. |
| `transcripts\` | each agent run ends | The worker's, fix round's and reviewer's session transcripts, copied out before a sandbox is removed. |
| `result.md` | the worker ends | What the worker wrote about its change (copied from its clone). |
| `review-<round>.md`, `review.md` | a review ends | The reviewer's findings; `review.md` is the latest. Each saved file starts with a `Reviewer: <harness> (<model>)` line sbxm adds, then the reviewer's own text, which starts with `Must-fix findings: <n>` then `Risk: low\|medium\|high`. |

A review normally takes 6 to 12 minutes after its sandbox has started. The command's own output says which stage it is
in (gates, `review round 1`).

**Cleaning up.** Each sandbox that builds the project keeps its own `target\`, which is 3 to 4 GB. `sbxm rm <project>
--purge` removes every sandbox of a project and its workspace (asks first); `sbxm task rm --issue <n>` does it
for a task. Check `sbx ls` for stopped leftovers.

## Troubleshooting

- **Start with `sbxm doctor`.** Each failure says what's wrong and how to fix it.
- **`sbx` says `try: sbx login` although you are logged in:** the drive `sbx` keeps its sandboxes on may be full.
  `sbxm doctor` shows its free space; remove sandboxes you no longer need (`sbxm rm`, `sbxm task rm`).
- **`sbx create` fails with `failed to run sandbox container`:** check the workspace isn't on a drive `sbx` can't
  mount (on the development machine, anything on `C:`), or under `%TEMP%`/`AppData`.
- **A host is blocked:** add it to `network.allow`, then `sbxm open <project> --harness h --rebuild`.
- **`secret '…' (secrets.services) is not stored in sbx`:** `sbx secret set <service>`, or `sbx setup` to import it
  from your environment.
- **Pi answers `401`:** approve the credential binding (see *Harnesses*).
- **`sandbox … exists but sbxm has no state for it`:** it wasn't created by sbxm here; remove it with `sbx rm` (this
  deletes its session history) and run `sbxm open` again.
- **A profile change isn't taking effect:** sbxm reads the copy in `profiles_dir`, not the one in the repo. Run
  `just deploy-profiles` (it refuses to write if `config.toml` is invalid), then rebuild the sandbox.
- **`git` fails with `Permission denied` on Windows (`.git/config`, `.git/objects/…`):** antivirus or the file indexer
  briefly holds a file git just created. It passes by itself, so run the command again.
- **A worker fails at once with "OAuth session expired and could not be refreshed"** (in its transcript's `result`;
  `sbxm task` may report it as "claude reported success, 0 commit(s)"): first refresh the stored login (`sbx setup`
  for Claude). Then test a plain sandbox: `sbx create --name auth-check claude <empty dir>`, `sbx exec auth-check
  claude -p pong`, `sbx rm auth-check -f`. If the plain one answers and the task's still fails, the profile's
  template has aged: run `sbxm template build <profile> --harness <h>` (about 15 minutes warm), then start the task
  again. A fresh task is needed, not `task resume`: resume reuses the sandbox that can't sign in.
- **`sbx exec`/`sbx run` print `context deadline exceeded` after the sandbox was created:** the sandbox exists; only
  the attach step timed out in a non-interactive shell. Check `sbx ls`.

## Development

`just check` runs formatting, lints, the Rust tests and the script (Pester) tests, with no Docker needed;
`just real-test` runs the tests against the real `sbx`. See [The `justfile`](#the-justfile) for every recipe. Design decisions are numbered in `sdlc/decisions.md`, the milestone plan is `sdlc/milestone-1.md`, and
`AGENTS.md` describes the code layout and workflow for coding agents.

### Working on issues with `sbxm task`

`sbxm task` hands GitHub issues to worker agents, one sbxm sandbox per issue, checks and reviews their work, and
opens the PR. The host picks the issues, so no two workers get the same one: it skips questions, issues whose
**Depends on** issues are still open, and issues **Related** to one that already has a task. The sandbox has no
GitHub access; the agent commits locally and writes `.sbxm-task/result.md`, and `sbxm` collects the commits through
a verified `git bundle` into its own repo (`<base_dir>\.sbxm\tasks\<id>\repo.git`), onto a separate ref for the
agent's own commits; before gates, review or finish see them, `sbxm` rewrites every one of those commits onto the
delivered branch with any closing reference or mention neutralised, rebuilding them unsigned; a commit over 1 MiB
(header plus message) is refused instead, so ask the agent for a shorter message. Only `sbxm` pushes.
The table in [Commands](#commands) lists every `task` command and flag.

A worker never reviews its own change. `task review` first runs the **gates** (`cargo fmt --check`, clippy and
`cargo test` in the worker's sandbox by default, plus any host commands you list), then a fresh reviewer in its own
sandbox, on its own clone so it can't change the branch, writes `review.md`. The reviewer's harness differs from the
worker's by default (Codex when the worker is Claude), so it doesn't share the worker's blind spots. If the review
has must-fix findings, the worker gets a round to fix them and the gates run again, as many times as `[worker]
fix_rounds` allows (default 3; a gate failure spends a round too). Each round after the first reviews only the
commits made since the last one, told which findings the previous round raised and asked to mark each finding as
repeating one of them or new; once one of those finds nothing, one more review of every commit confirms it before
the task is `ready`. If the same must-fix finding, in the same file, keeps coming back after a fix round, the task
stops at once instead of spending the rest of the round budget on it. Whatever is still open once it stops goes into
the PR description, and `task finish` opens that PR as a draft (the task's `stopped` field says why, `rounds-exhausted`
or `repeat-finding`; a task left with must-fix findings is a draft too); nothing is filed as
an issue unless you run `task file-findings`. To keep going instead, `task resume --issue <n> --rounds 1` gives a
stopped task one more fix round (a `repeat-finding` stop with rounds left needs no `--rounds`).

Every change to sbxm goes through a PR, including ones not made by workers, and gets the same review:
`task review --pr <n>` fetches the PR's head, runs the gates in the reviewer's sandbox, and posts the review as a
comment on the PR. There's no fix round; the author fixes the findings and runs it again, in place: the earlier
round's review stays in the task folder, the gates run again for the new commits, and the reviewer gets every
earlier round as context, opening its comment with the status of each earlier finding (fixed, still open, or
deferred to an issue). An unchanged head refuses, naming the commit and the `--force` command to review it again
anyway. PRs from forks are refused, because the code would run on your machine. A PR that `task finish` opened
already has its review: the in-task review, posted as a comment naming the reviewed commit, so `task review --pr`
refuses that head (`--force` reviews it anyway) and only a later commit needs it.

One-time setup:
- `just deploy-profiles` copies `profiles/sbxm-dev` into your `profiles_dir` (again after the profile changes). It
  installs Rust and a C toolchain with `pkg-config` and the OpenSSL headers (`openssl-sys` needs them), allows
  crates.io and `cdn.pyke.io` (the ONNX Runtime download of the `fastembed` build), and needs the `anthropic` secret;
  a Codex reviewer also needs `openai` (`sbx secret ls`). It leaves out PowerShell, Pester and `just` to keep sandbox
  setup short, so `just script-test` runs on the host only. A worker should not install software itself: what a build needs belongs in the
  profile's `setup.install`, or a clean sandbox (such as a PR review's) fails where the worker's passed.
- `sbxm task init` writes `sbxm-task.toml` in the repo's root: the worker, reviewer, profile, gates and time limits.
- `[risk]` in `sbxm-task.toml` adds to the risk assessment's built-in path-rule floor (decision 176(d)): `medium =
  [...]` and `high = [...]` list patterns (matched case-insensitively against a changed path); `replace = true`
  drops the built-in defaults instead of adding to them. Put this repo's own sensitive paths (e.g. `src/git.rs`,
  `.sbxm-task/`) at `high` here.
- `gh auth status` must show you logged in, and git must be able to clone and push without a prompt: run
  `gh auth setup-git` once. `sbxm` never lets a credential prompt hang; it fails and names that command.

```powershell
sbxm task start --workers 2     # pick up to 2 issues, one sandbox each, wait for the workers
sbxm task status                # stage, status, commits ahead of base, result and review present
sbxm task review --issue 1      # gates, independent review, fix rounds up to the budget
sbxm task finish --issue 1      # push issue-1 and open a PR with "Fixes #1", the result and the review
                                # (an issue of an open PR: push to that PR's branch and comment on it)
sbxm task rm --issue 1          # after merging: the sandboxes, clones and record (asks first)
sbxm task review --pr 12        # review any open PR of this repo and comment the result on it
                                # (refused for a head the in-task review covered; --force overrides)
sbxm task run --issue 1         # start and review in one go, stopping before finish
sbxm task run --spec idea.md    # the same, for a spec task; no GitHub call anywhere
sbxm task run --issue 210 --issue 202 --spec sdlc/specs/x.md   # a list, run one after another
sbxm task resume --issue 1      # continue a failed or interrupted task where it stopped
sbxm task resume --issue 1 --rounds 2   # two more fix rounds for a task that stopped ready
```

Workers run for up to 2 hours and reviewers for 45 minutes by default (`--time-limit`, `--reviewer-time-limit` or
`sbxm-task.toml`). Each task keeps `task.json` (its stage, status, gate results, the sandbox names, and for an issue
task the fix round used so far, the round budget and why it stopped if it did), `gates.log`, `issue.md`, `result.md`,
`review-<round>.md` (`review.md` is the latest) and the transcripts under `<base_dir>\.sbxm\tasks\<id>\`; the clones
are `<base_dir>\tasks\<id>`, `<id>-review` and `<id>-gates`. A task killed halfway shows as `interrupted` in
`task status`; `task resume --issue <n>` continues it from that stage, keeping its clone and commits (it also
retries a stage that failed), and `task start --restart --issue <n>` (after the same confirmation as `task rm`)
starts it over instead.

Sandbox setup takes about 1 minute per sandbox (apt 44 s, rustup 13 s, measured 2026-10-11). `base_dir` only moves the
workspace: each sandbox's own disk lives in the folder `sbx` keeps its state in, on the system drive, and `sbx` has no
setting to move it (a junction to a bigger drive was tried and failed for the `claude` agent; see [`docs/workflow.md`](docs/workflow.md)).
Budget about 7 GB for a fresh sandbox and much more once a large build runs in it: the disk image
is up to 20 GB for `/` plus a 10 GB Docker volume, and the debug builds of the gates (`check`, `clippy`, `test`
share one target folder) reached 17 GB for a crate that pulls in `fastembed`. A full disk shows up as a gate that
fails with exit 101 and no failing test, so keep over 10 GB free before starting a sandbox and run one at a time
unless 25 GB or more is free. `CARGO_PROFILE_DEV_DEBUG=0` in the `[gates]` commands drops debug info and should
shrink the builds (its effect on this repo's builds was not measured).
Deleting files inside a sandbox does not return space to the host; removing the sandbox (`task rm`) does.
`sbxm doctor` checks the free space of both the base dir and the drive `sbx` keeps its sandboxes on, and the commands
that create a sandbox refuse under `min_free_disk` (see *The sandbox drive*).

#### Running tasks in Docker Cloud Sandboxes

A repo can run its `sbxm task` sandboxes, workers and reviewers, in Docker Cloud Sandboxes (`sbx --cloud`) instead
of on this machine (milestone 4, decisions 252-265). `sbxm run`, `new` and `open` always run here.

```toml
# sbxm-task.toml
[sandbox]
profile = "sbxm-dev"
location = "cloud"   # or "local", the default when the key is absent
```

Any other value is an error naming the file (`` `remote`, expected `local` or `cloud` ``). `--cloud` or `--local`
overrides the key for one command, and only the commands that start a task take them: `task start`, `task run` (for
every task of a list) and `task review --pr N`. The task records its location in `task.json` (`"location":
"cloud"`; a record without it is local), and every later command (`task review --issue/--spec`, `task resume`, `task
gates`, `task finish`, `task rm`, `task status`) uses the recorded location, whatever the key says by then. sbxm
never falls back to a local sandbox by itself.

What changes in the cloud:
- **No mount.** Each sandbox is created by `sbx --cloud create` with the profile's two kits, a `--ttl` and
  `--on-timeout delete`. The task branch goes in as a `git bundle` made from the task's `repo.git`, copied in with
  `sbx --cloud cp` and cloned under `$HOME/work/<id>` (the path is recorded in `task.json`); the prompt files go in
  the same way. `result.md`, `review.md` and the bundle of the agent's commits come out by `cp` into a fresh host
  staging folder, each checked to be one regular file under its size cap before it is read, and the bundle is
  verified before it is fetched, as locally. Host-tier gates are unchanged. Build under `$HOME` in your gates, never
  `/tmp` (see [`docs/workflow.md`](docs/workflow.md)).
- **No templates.** A profile with `[template]` is built from its kits in the cloud, with one line saying cloud
  templates aren't used.
- **A strict profile** (`strict = true`) is created with its build allow list so its install steps run; then, before
  any agent runs, sbxm denies in that one sandbox (`sbx --cloud policy deny network --sandbox <name> <host>`) every
  allowed host outside the runtime list: the harness's provider hosts (for `claude` `api.anthropic.com`,
  `claude.ai` and `console.anthropic.com`; for `codex` its OpenAI hosts) plus `[template].allow`. The denied hosts go
  to the task's `run.log` (`strict cloud sandbox <name>: sbxm denied <hosts>`). It then reads `sbx --cloud policy ls`
  again: a host still allowed outside the runtime list, or a list it can't read, removes the sandbox (then its scoped
  secrets) and refuses, naming the host. This holds for worker, reviewer and fix-round sandboxes alike. A strict
  profile with a harness other than `claude` or `codex` is refused before anything is created.
- **Secrets.** The provider logins stay global in the cloud: `sbx secret set anthropic --oauth --cloud` and `sbx
  secret set openai --oauth --cloud`. Their values never enter a sandbox. No other secret has a value source in the
  cloud yet, so a profile whose `secrets.services` names anything else is refused for a cloud task.
- **No free-disk check.** The `min_free_disk` check of the sandbox drive is skipped for a cloud task.
- **`task start --workers N`** works as it does locally.

**Before anything is written**, a cloud command (`task start`, `task run`, `task review`, `task resume`, and `task
gates` when it has to make the worker sandbox again) checks the harness and profile of each cloud sandbox it may
create in this order, and refuses at the first failure, naming the fix and ending with "or run it here with
`--local`":

| Refused when | The message names |
|---|---|
| `sbx` is older than 0.46.0, the cloud minimum | the version found and 0.46.0: upgrade `sbx` |
| the cloud doesn't answer `sbx --cloud ls --json` (not logged in, for example) | `sbx login` |
| the harness's provider login can't be stored in the cloud (only `anthropic` and `openai` can, so a `pi` or `antigravity` harness is refused) | a `claude` or `codex` harness |
| a profile secret with no cloud value source: any `secrets.services` entry other than `anthropic` or `openai` | the secret and the profile: take it out of the profile |
| a provider login the task needs is missing in the cloud: the harness's own, or any provider the profile names (a `codex` worker whose profile names `anthropic` needs both) | `sbx secret set <provider> --oauth --cloud` |

`task run` checks the reviewer's sandbox too before the worker starts, and a review that may still run a fix round
also checks
what making the worker sandbox again would need (its recorded profile, a committer identity in git's `user.name` and
`user.email`, its TTL). A failure of the cloud in the middle of a task (`sbx --cloud` not answering, an `exec`
stream error without `timeout -v`'s lines) stops the task the way `sbx` not answering does locally, and `task resume`
continues it in the cloud. A stream error after `timeout -v`'s lines is a timeout, as locally.

**Lifetimes.** Every cloud sandbox has a TTL, rounded up to whole minutes:
- the worker sandbox: `[worker] time_limit` + every `[gates] sandbox` command's timeout + `fix_rounds` x
  (`[reviewer] time_limit` + `[worker] time_limit` + every sandbox gate's timeout) + 30 minutes (14h45m with the
  defaults and three gates), where `fix_rounds` is the task's own budget: the config's at its start, plus any
  `task resume --rounds N` (a `--rounds` whose TTL doesn't add up is refused before anything changes), never what
  `sbxm-task.toml` says later. It lives through the task's automatic run (worker, sandbox gates, review rounds, fix
  rounds) and is removed when the task stops, `ready` or stopped for any reason, so nothing is billed while the task
  waits for you;
- a reviewer sandbox: `[reviewer] time_limit` + 30 minutes (1h15m by default), one per review round, removed after it.

A removal that `sbx` answers with "temporarily unavailable" is retried after 5, 10 and 20 seconds; a sandbox already
gone counts as removed, and one that still can't be removed is a warning naming `sbx --cloud rm <name> -f`.
`task.json` keeps a `sandboxes` list, one entry per cloud sandbox (`name`, `role` `worker`/`reviewer`/`fix`,
`location`, `created_at`, `ttl_s`, `removed_at`, null while it exists or when its removal failed), saved once its
create has returned, and `run.log` a line at each create (`cloud sandbox <name> created (<role>, TTL <ttl>)`) and
each removal (`cloud sandbox <name> removed`). **sbxm never deletes what it can't prove it made (decision 268):** a
cloud sandbox is the task's own only when its create returned success and its entry was saved. Before any cloud
create (the worker at the start, each reviewer, a worker made again) sbxm checks `sbx --cloud ls` and refuses a name
already there, before any secret is scoped to it (`cloud sandbox <name> already exists and isn't one this task
made; remove it with sbx --cloud rm <name> -f if it is nobody's`), so its secret values never go to a name another
caller holds; a worker sandbox still listed after the task removed its own is refused the same way instead of being
reused. That check is not ownership: names are deterministic (`sbxm-task-<id>-<harness>`), and another caller can
make the name between the check and the create. So `task.json` records the create as started (`creating`) before its
secrets are set and the create asked for, and clears it once the create returns. An `sbxm` killed in between leaves
an **ambiguous create**: sbxm can't tell whether the cloud made that sandbox for this task or someone else made one
of the same name. Every recovery and removal path (`task resume`, `task finish`, `task rm`, `--restart`, `task rm
--merged`) then leaves that sandbox and every secret scoped to its name alone, and refuses to delete or archive the
task (`task finish` publishes and warns), naming the sandbox and the commands: look with `sbx --cloud ls` and `sbx
--cloud secret ls`, and if it is the task's, remove it with `sbx --cloud rm <name> -f` and each secret with `sbx
--cloud secret rm <service> --sandbox <name> -f`. Then tell sbxm it's handled: `task rm ... --forget-ambiguous` (or
`task rm --merged --forget-ambiguous`) drops it from `task.json` without any cloud call for it, before anything is
removed or archived, then removes the rest; if that removal stops part way, a plain `task rm` finishes it. `task resume ... --forget-ambiguous` drops it and resumes (a sandbox of that name still there is then refused like
any other name the task didn't find free). `task gates`, `task review` and a fix round refuse before using its sandbox
too, with no cloud call. A PR task can't be resumed: run `task rm --pr N --forget-ambiguous`, then `task review --pr
N`. Its TTL deletes an ambiguous sandbox eventually anyway. A create that
returned an error takes back the secrets it set and stops the task, so nothing of that name is removed later. Add up
the entries for a task's sandbox hours: `task rm --merged` archives them with `task.json` and `run.log`, a plain
`task rm` deletes them with the folder.

The other commands of a cloud task:
- `task review --issue/--spec`: a reviewer sandbox per round in the cloud; a fix round runs in the task's worker
  sandbox, made again from a bundle of the branch if it is gone.
- `task review --pr N --cloud`: the PR's head goes in as a bundle from `repo.git`.
- `task resume`: always in the cloud. The preflight runs again first; a worker sandbox that is gone (its TTL ran
  out, a failure) is made again from the recorded profile and a bundle of the branch.
- `task gates --issue N`: runs the sandbox tier in the worker sandbox at the recorded clone; a sandbox that is gone
  is made again first and removed when the gates end.
- `task finish`: host-side only (`repo.git`, GitHub); afterwards it removes any cloud sandbox the record still holds
  open (one whose removal at the task's stop failed), retried, and the secrets scoped to it.
- `task rm` and `task rm --merged`: remove each cloud sandbox the record still holds open (`sbx --cloud rm`, retried)
  and the secrets scoped to it, then the clones and the folder. While a cloud sandbox may still be running, the
  task's folder stays: its record is the only trace of that sandbox.
- `task status` shows each task's `location:` (`local` for a record without one); `sbxm status` adds a table of
  the running cloud sandboxes named `sbxm-` (`CLOUD SANDBOX`, `STATUS`, `SHAPE`, `LEFT`); `sbxm doctor` lists each
  one with its time left. When the cloud can't be read, `status` prints one line instead and still exits 0, and
  `doctor` warns instead of failing. `doctor` also warns when `sbx` is older than 0.46.0 while the cloud is in use
  (the current repo's `sbxm-task.toml` says `location = "cloud"`, or a task record says `cloud`), and when the
  current repo is a cloud repo whose `[gates] sandbox` commands name `/tmp`.

**Cost.** Cloud sandboxes are billed by Docker. Every one has a TTL with `--on-timeout delete` and is removed when
the task stops, but a removal can fail and a TTL can be long, so check `sbxm status` or `sbxm doctor` for running
ones (or `sbx --cloud ls`), and read your bill. There is no budget limit in sbxm.

[`docs/workflow.md`](docs/workflow.md) shows how the commands fit together, with the use cases and these practical
notes in one place.

### Filing review findings as issues

Reviews run in sandboxes without GitHub access, so their findings sit in `sdlc/reviews/<date>-<scope>.md` marked
`Issues: pending`. `sbxm task file-findings` files one issue per finding, from a host where `gh` works. It is a dry
run unless you pass `--create`, so read the dry run first: it prints every issue body, and path and secret scrubbing
is heuristic (personal paths become `~`; a secret-looking line refuses the finding).

```powershell
sbxm task file-findings --file sdlc/reviews/2026-09-30-milestone-2a.md                      # dry run
sbxm task file-findings --file sdlc/reviews/2026-09-30-milestone-2a.md --create             # file the issues
sbxm task file-findings --file sdlc/reviews/2026-09-30-milestone-2a.md --create --only M2A-1 # just these ids
sbxm task file-findings --pr 12 --create                                                    # a task's own review.md
```

`--standard-criteria` files a finding that has no acceptance criteria with only the standard ones, `--keep-paths`
keeps personal paths as they are, and `--repo <owner/name>` overrides the repo taken from `origin`. A rerun never
duplicates issues (a hidden marker in each body); it also fills in the `#n` links of issues filed earlier.
Afterwards the review's `Issues:` line is rewritten to `Issues: <id> #<n>, ...`. Decision 134 has the details; the
review format is described in the `sdlc-code-review` skill.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your
option.
