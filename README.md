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
  (`sbx login`).
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
| `sbxm run <config>` | Runs a comparison: checks the run-config and that every needed secret is stored in `sbx` (each contestant's provider, the judge's, and the profiles' `secrets.services`), builds and validates the kits, then gives each (contestant, repeat) pair its own throwaway sandbox. A contestant can set its own `profile`; otherwise `[run].profile` (or your `default_profile`) applies, and the judge uses the run's. Contestants of a repeat run in parallel; repeats run one after another. Each sandbox is removed afterwards, even after an error; the workspaces stay under `<base_dir>/runs/<run-id>/<contestant>/<repeat>/`. Prints the run ID and one line per pair (completed, timed out or failed). With `task.seed`, each contestant gets its own copy of the seed as a fresh git repo with one baseline commit (the seed's history and remotes are dropped); without it, an empty plain folder. Each pair's diff (everything the contestant changed or added, even if it committed) is captured before its sandbox is removed. Each pair's results are saved the moment it finishes under `<base_dir>\.sbxm\runs\<run-id>\<contestant>\<repeat>\` (`answer.md`, `diff.patch`, `transcript.jsonl`, `result.json`), next to `run.json` (the run's identity: config hashes, profile, `sbx` version, times) and a copy of the run-config; `run.json` gets `completed_at` only once everything is saved. Any `[[eval.checks]]` run inside each contestant's sandbox after its agent finishes (and after its diff is taken), as `sh -c <command>` with a time limit enforced inside the sandbox; exit code 0 passes. The verdicts are saved in `evals.json` next to the pair's other files and summarised on the pair's line (`; checks 2/3 passed`). With `[eval.judge]` and a rubric, an LLM judge then scores the contestants: after all pairs are done, once per repeat, in its own throwaway sandbox, it sees each contestant's answer and diff under an anonymous label (A, B, ...) and scores every rubric criterion (`pass_fail` or a `scale` of levels). A judge from the same provider as a contestant is allowed, with a warning. Verdicts are saved in each pair's `evals.json` and per repeat in `judge/<repeat>/judge.json` (the label mapping); a judge that fails is warned about and doesn't fail the run. The contestants are then ranked: each criterion is 0 to 1, a contestant's score is the weighted mean over the criteria the judge scored (criteria it skipped are listed, never counted as 0), repeats are averaged, equal scores share a rank, and executable checks are shown alongside without changing the score. The ranking is printed before the last line, `Results: <folder>`. With `[eval.cosine]`, once every pair of a repeat is done, each contestant's non-empty text answer is compared with every other's using a local sentence-embedding model (`fastembed`, `all-MiniLM-L6-v2`), one repeat at a time, never mixing repeats. The model's five files (`model.onnx`, about 90 MB, plus `tokenizer.json`, `config.json`, `special_tokens_map.json`, `tokenizer_config.json`) must already sit in `model_dir` (default `<config dir>/models/all-minilm-l6-v2`): fetch them by hand from `https://huggingface.co/Qdrant/all-MiniLM-L6-v2-onnx/resolve/main/`, since `fastembed`'s own downloader uses bundled roots and fails behind a TLS-intercepting proxy. A missing file is refused before anything is written, naming the file, the folder and that URL. The similarity (-1 to 1, 1 meaning near-identical wording) is saved per pair in `evals.json` and printed, two decimals, as one line per repeat after the ranking, with a `no comparable answers` line instead for a repeat whose saved cosine data has no numeric pair to show (every participant errored, was skipped, or only one answered); it's a measure of agreement between contestants, not of quality, so it's shown alongside the ranking and never changes it. A missing or empty answer is skipped, not an error; a model that fails to load, or that loads but fails while embedding, is one warning for the whole run and an `error` entry per pair instead of failing the run. |
| `sbxm run show <run-id> [--diff]` | Prints a saved run from its files: when it started and completed, the profile and `sbx` version, and for each contestant and repeat the status, the answer, a summary of the diff (files changed, lines added and removed) each executable check's verdict and the judge's scores, with the anonymous label each contestant was judged under. `--diff` also prints every full patch. With a judge it also prints the ranking, recomputed from the saved files (edit `run-config.toml` in the run folder to try other weights); with `[eval.cosine]` it also prints the same similarity lines `sbxm run` printed, read straight from the saved `evals.json` files without loading the model. A run that is still going or was interrupted shows what has been saved so far. Only reads files; the id must look like `2026-09-30-a1b2c3`. |
| `sbxm task init [path]` | Writes a starter `sbxm-task.toml` (the worker, reviewer, sandbox profile and gates for `sbxm task`) in the repo's root. Valid as written for a Rust repo. Refuses to overwrite. |
| `sbxm task states` | Prints the task state machine's transition table (from, event, to, action), generated from `src/task/machine.rs`'s `TABLE` so the table and the code can't drift apart (checked against `sdlc/specs/task-state-machine.md` section 5.2 by a test). Reads nothing and calls no backend. |
| `sbxm task start (--issue N... \| --workers N \| --spec FILE) [flags]` | Hands GitHub issues to worker agents. For each chosen issue it makes a task: its own sandbox (`sbxm-task-issue-<n>-<harness>`), a clone of the repo on branch `issue-<n>`, and a headless worker that follows the issue and commits. `--workers N` picks up to N open issues (`must-fix` before `should-fix`; it skips questions, issues whose `**Depends on:**` issues are still open, issues that already have a task and issues `**Related:**` to one that does); `--issue N` starts exactly the issues named (one that is blocked by an open issue is still refused). An issue whose body starts with `PR: #m` (a review finding, see `task file-findings`) belongs to that PR: when PR m is open and a branch of this repo, its task starts from the PR's branch head (fetched into the task's own repo) instead of the base branch, in a fresh sandbox, the worker's prompt says which PR it is and that the branch already has commits, and `task.json` records `continues`: the PR, its branch and the commit it started from (`base`); a closed or merged PR, or one from a fork, is refused before anything is written. The tasks run in parallel and one failing doesn't stop the others. The worker has no GitHub access: the issue text is copied into its clone. When it stops, its commits are collected into a bare repo that only sbxm and your git touch (`<base_dir>\.sbxm\tasks\<id>\repo.git`) through a verified `git bundle`: the bundle must be a plain file under 500 MB, only the task branch is fetched, and no git command ever runs inside the agent's folder. Also saved there: `issue.md`, `result.md` (what the worker wrote), the transcript and `task.json`. Flags override `sbxm-task.toml`: `--worker-harness`, `--worker-model`, `--time-limit`, `--profile`, `--base`, `--repo`. `--restart` (with `--issue`) first deletes an existing task of that issue, showing what and asking like `task rm` (`--yes` skips the question), after checking that the issue is still open, the task isn't running and every other input is valid. After the worker (unless it failed) the task's gates run (see `task gates`); then `task review` and `task finish` (or `task run` for start and review in one go). Exits non-zero if any task or its gates failed. **`--spec FILE` instead starts a task from a file** (a PRD or spec, not a GitHub issue): the task id is `spec-<name>-<hash>` (the file's sanitized name plus 6 hex digits of a hash of its canonical path, so two files named alike never collide), the file is copied in as `source.md` in place of `issue.md`, and the worker's prompt points at that instead. No GitHub call is made anywhere in this path, so an omitted `--base` is read from this checkout's `origin/HEAD`; if `--repo` names a repo other than the checkout's origin (or the checkout has no `origin/HEAD`), pass `--base <branch>`. It otherwise runs the worker and gates exactly like an issue task; continue it with `sbxm task review --spec FILE`. `sbxm task finish` is refused for it until a sink (`local`/`push`) is built, in the issues that follow it; `--restart`, `--workers` and continuing an open PR don't apply to it. |
| `sbxm task review (--issue N \| --pr N \| --spec FILE) [--repo owner/name] [--base b] [--reviewer-harness h] [--reviewer-model m] [--reviewer-time-limit 45m] [--time-limit 2h] [--profile p]` | Has a task's change reviewed by an independent agent, then lets the worker fix what it found, as many times as `[worker] fix_rounds` in `sbxm-task.toml` allows (default 3). Checks first, before anything is created: the task is ready for review, and the reviewer's provider secret (and the profile's) are stored. The gates run first unless they passed since the last change (a failure feeds a fix round too, counted against the same budget, with the gate's own output in the fix prompt instead of a review; once the budget is used with a gate still failing, the review stops there, exit 1). The reviewer runs in its own sandbox over its own clone of the task branch (made from the host-owned repo, so it sees exactly the collected commits and can't touch the worker's folder), told to follow the repo's `sdlc-code-review` skill, to change nothing and to write `.sbxm-task/review.md` whose first line is `Must-fix findings: <count>`; Codex runs with high reasoning effort. The review is saved under the reviewer's name (`Reviewer: codex (model)`) as `review-<round>.md` and `review.md`; a review without that first line is kept for reading but not used. The first review (and any after the budget is spent and must-fix findings remain) covers every commit on the branch; if it counts must-fix findings and a round is still in the budget, the worker gets a fix round in its own sandbox (the review and a fix prompt are put in its folder; its new commits are collected like the first time), the gates run again, and the next review is scoped only to the commits made since the last one. Once a review like that finds nothing, one more review of every commit runs to confirm before the task is `ready`. From round 2 on, the reviewer sees every earlier round's review, not only the one right before (so a narrow round that misses nothing new can't hide an older finding that is still open), and is asked to mark each finding `Repeat of: <id>` (naming the earlier finding it repeats, in the same file — either side may list more than one, line numbers aside) or `new`; a must-fix finding that validly repeats one stops the task at once (`stopped` = `repeat-finding`), even with rounds left in the budget, because fixing it clearly isn't working — a claim naming a different file, or an id no earlier round has a must-fix finding for, counts as new instead and never stops it by itself, with a warning so the wrong claim isn't silently dropped. The reviewer's sandbox and clone are removed after each round, also after an error. The task ends `ready`; must-fix findings left once the round budget runs out are reported, not an error, and the task's `stopped` field is set to `rounds-exhausted` (or `repeat-finding`, above); `task status` shows it. A reviewer like the worker's harness warns (less independent). For an issue's task nothing is pushed or posted. **`--pr N` instead reviews an open pull request from a branch of this repo** (a fork's PR is refused, as is a closed or merged one, before anything is created): the PR's head is fetched into the host-owned repo (with git's object check on), the PR's description and the issues it closes are the reviewer's context, and the gates run on a clean checkout: the sandbox tier inside the reviewer's own sandbox (a PR has no worker), the host tier as usual. The reviewer runs once, with no fix round (a PR task's `fix_rounds` is always 0), and a valid review is posted as a comment on the PR (`@mentions` are neutralised and a very long review is cut with a note; the whole text is `review.md` in the task folder). Nothing is posted if the gates fail or the review is invalid; if only the posting fails, the review stays saved and the error gives the `gh pr comment` command to post it by hand. A PR task that exists is retried only when its review failed; to review a PR again after it changed, remove its task first with `sbxm task rm --pr N`. `--repo` and `--base` apply to PRs (defaults: this checkout's origin, the repo's default branch). **`--spec FILE` instead continues a spec task** (started with `task start --spec FILE`): the same gates, review and fix-round loop as an issue's, with `review.md` its only output (no PR to comment on, no GitHub call made); the printed "ready" line notes that `sbxm task finish` is refused for it instead of suggesting it. |
| `sbxm task gates --issue N [--tier sandbox\|host\|all] [--dry-run]` | Runs a task's gates, the checks `sbxm` itself runs (an agent's prompt can't skip them) to decide whether its work may go on. They are listed in `sbxm-task.toml` under `[gates]`. The **sandbox tier** (`sandbox = [...]`, default for a Rust repo: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`) runs each command as `sh -c` in the worker's own sandbox, on its folder, under a time limit enforced inside the sandbox (`timeout = "20m"` per command); the first failure stops the tier. The optional **host tier** (`host = [...]`, off while empty) runs on this machine, in a clean checkout of the task branch from the host-owned repo (only committed files, no hooks), only after the sandbox tier passed, and the checkout is removed afterwards. **Host gates run agent-written code on your machine, outside any sandbox**: list only commands you would run on a stranger's pull request. Results go to the task's `task.json` and `gates.log`; a failure marks the task `gates-failed`. `task start` runs them after the worker; this command runs them again on demand, for example after you fixed something by hand in the worker's clone: first it collects what is *committed* there into the task's repo, so the host tier, `task review` and `task finish` use the same commits as the sandbox tier (commit your fix; uncommitted changes are seen by the sandbox tier but are never collected). `--dry-run` prints each tier's commands, where they run and whether the tier is off, and changes nothing. A run of one tier is partial: `task review` skips its own gate run only if the passing runs together covered every configured tier on the task branch's current commit, and `--tier host` is refused unless the sandbox tier passed on that commit. Refused while the worker is running or after the task has moved past its gates. |
| `sbxm task status [--issue N \| --pr N] [--json]` | One line per task: stage, status (`interrupted` when its `sbxm` process is gone), `ahead:` the commits on the task branch past its base in the task's own repo (`-` before the repo exists; for a task that continues a PR, past the commit the PR's branch was at when the task started, so the PR's earlier commits don't count), and whether `result.md` and `review.md` exist; a task that continues a PR ends with `[PR #m (<branch>)]`, and a task that stopped itself ends with `stopped: <reason>` (`rounds-exhausted` or `repeat-finding`). `--json` prints the records, plus `interrupted` and `commits_ahead` (`null` when unknown). |
| `sbxm task file-findings (--issue N \| --pr N \| --file F) [--create] [--only ID,...] [--standard-criteria] [--keep-paths] [--repo owner/name]` | Files the findings of a review as GitHub issues, one per finding. `--file` takes any review file (e.g. `sdlc/reviews/<date>-<scope>.md`); `--issue`/`--pr` take the `review.md` of that task (the built-in reviewer prompts ask for the skill's review format, with an `Issues:` line; a custom `[prompts] reviewer` must do the same). **A dry run unless `--create`**: it prints every issue it would create and the `Issues:` line it would write, and changes nothing. Before any write it checks the finding ids, that the review has exactly one `Issues:` line, your `gh` login, the repo and its labels (`must-fix`, `should-fix`, `question` must exist; none are created), and every finding: a secret-looking line (token, key, password assignment) refuses the whole run, naming the finding and line but never the value, and personal paths (`C:\Users\<name>`, `/home/<name>`) become `~` (`--keep-paths` keeps them). A must-fix or should-fix finding with no acceptance criteria is refused unless `--standard-criteria`. Each issue is labeled `must-fix`, `should-fix` or `question` by its section; with `--pr N`, every body's first line is `PR: #N`. Issues are created in dependency order with permalinks at the reviewed commit and `Depends on`/`Related` links to the other issues; a hidden marker in each body means a rerun never files a finding twice (it skips it and reuses its number), and the links of issues that already exist are patched in those two fields only. A finding also skips, naming the issue that already covers it, when an open issue has the same title and PR number even without the marker (decision 169); an exact marker match, in any state, always takes precedence over that title-and-PR fallback, so continuing a PR's review doesn't duplicate what was already filed; a closed one doesn't count. Such an adopted issue, and an open issue matched by its marker, gets the finding's label, replacing another of the three (other labels stay): the dry run prints `S-1 skipped (exists #77), would label it must-fix (removing should-fix)` and `--create` reports `skipped (exists, labeled must-fix)`. With `--pr N`, an open issue matched by its marker also gets its body's first line set to `PR: #N` if that line is missing or stale, alongside its link-field patch; the dry run names the pending update (`would set its body's first line to PR: #7`) and changes nothing. Afterwards the review's `Issues:` line is rewritten (`Issues: S-1 #41, S-2 pending`). Exits non-zero if some issue could not be filed; rerun to file the rest. |
| `sbxm task finish (--issue N \| --spec FILE)` | Publishes a ready task: pushes branch `issue-<n>` from the task's host-owned `repo.git` (never forced) and opens the PR from it to the task's base, titled like the issue, with `Fixes #<n>`, then `result.md` and `review.md` each under a heading (a file over 25,000 characters is cut, and the body and the output say so). Every check comes before the push: the task must be `ready` and have no PR yet, the repo, branch and base names must be usable, the branch must have commits beyond the base, and `result.md`/`review.md` must not contain a secret-looking line (refused, naming the line, never the value). The task then becomes `finished` and `task.json` keeps the PR's URL. If the push worked but opening the PR failed, the task stays `ready` with a note and the error says so: run `finish` again, which pushes the same commits (nothing to do) and retries only the PR. If someone opened the PR by hand in between, GitHub's refusal is shown and nothing is recorded. **It refuses to push a branch that adds, edits or deletes anything under `.github/workflows/` or `.github/actions/`** (letter case ignored): GitHub would run those files with the repository's secrets the moment the branch is pushed, before anyone has read what the agent wrote, so read them in the task's `repo.git` and push the branch yourself if the change is meant. **A task that continues an open PR** (its issue starts with `PR: #m`, see `task start`; no extra flag) is published differently: its commits are pushed to PR m's own branch and no PR is opened; instead PR m gets a comment listing the commits added (short id and subject) and `Fixes #<n>` for the issue (a subject over 500 characters is cut, and commits beyond what fits in one GitHub comment are counted instead of listed; the comment says so). The push is a fast-forward from the commit the task started at, never a force: it is refused, with the reason and before anything is pushed or posted, unless PR m's branch on GitHub is still at that commit (or already at the task's own commits, when a rerun only retries the comment and pushes nothing). The push itself checks that again atomically, carrying that commit as the expected old value (`--force-with-lease` pinned to it, only ever for a fast-forward), so a branch that moves between the check and the push is left as it is and the push is refused; if someone pushed to it meanwhile, or it was deleted, start the task again with `task start --restart --issue <n>`. The workflow check and the commits-needed check count only the task's own commits, past that start commit. If the push worked but the comment failed, the task stays `ready` with a note and `finish` again retries only the comment. The task then becomes `finished`, `task.json` keeps PR m's URL, and the re-review is `sbxm task review --pr m`, unchanged (after `sbxm task rm --pr m` if PR m was reviewed before). **`--spec FILE` is always refused**, with a message naming why: a spec task has no sink (`local`/`push`) to publish through yet, in the issues that follow issue 142. |
| `sbxm task rm (--issue N \| --pr N) [--yes]` | Deletes a task: its sandboxes (the worker's and the reviewer's, by the names saved in `task.json`), its clones (`<base_dir>\tasks\<id>`, `<id>-review`, `<id>-gates`) and its folder `<base_dir>\.sbxm\tasks\<id>\`. It first prints the exact sandboxes and paths and asks `[y/N]`; without a terminal it refuses unless you pass `--yes`. It refuses while the task is running (an interrupted one can go), and builds paths only from an id that looks like `issue-41` or `pr-7`. It never follows a link or junction: a task folder that is one, or that resolves anywhere but its own place under the base folder, stays (a link *inside* a clone is removed, not followed). Removal carries on after a failure and says what went and what stayed; the task folder is deleted last and only if everything else went, so a second `rm` still knows the sandbox names. Exits non-zero if anything stayed. |
| `sbxm task run --issue N [flags]` | `task start` and then `task review` for one issue, with the flags of both (`--worker-harness`, `--worker-model`, `--reviewer-harness`, `--reviewer-model`, `--time-limit`, `--reviewer-time-limit`, `--profile`, `--base`, `--repo`, `--restart`, `--yes`). The reviewer's flags are checked before the worker starts. It stops at `ready` and prints the `task finish` command: publishing stays a separate step. Exit 1 for any failure, including failing gates; a task that ends `ready` with must-fix findings left is reported, not an error. |
| `sbxm config show [project] [--profile p] [--harness h] [--kits]` | Prints the merged config exactly as it's hashed, the hash, and with `--kits` the generated kits. Creates nothing. |
| `sbxm config profiles-dir` | Prints the folder profiles are read from (`profiles_dir`, else `<config dir>/profiles`), for scripts such as `just deploy-profiles`. Loads only `config.toml`, so it fails like any command on an invalid or missing one. |
| `sbxm doctor` | Checks `sbx` (on `PATH`, new enough, daemon answering), the config, every profile, every project with each of its sandboxes (secrets stored, kits valid), and the base dir (exists, writable, not a temp folder, at least 10 GiB free). Exits non-zero if anything fails. |

`--harness` defaults to `claude` for `new` and `config show`; `open`, `stop` and `rm` (without `--purge`) require it, so they never act on the wrong sandbox by default (issue #33). `sbxm <command> --help` shows every option.

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
| `antigravity` | Antigravity (`agy`) | `~/.gemini/AGENTS.md` | Uses the Antigravity kit from Docker Hub, pinned to a fixed tag. With a `google` secret stored (`sbx secret set -g google`), `agy` uses it as a Gemini API key and needs no sign-in (decision 137). `sbx`'s skills store isn't known to serve it: sbxm warns unless `skills.store = "off"`. |
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

[resources]            # per sandbox; sbx would otherwise take all CPUs and 16 GiB
cpus = 4
memory = "8g"
```

There's no `default_harness` setting: use `--harness`.

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

## Config changes, drift and `--rebuild`

Every sandbox records a hash of what it was built from: harness, profile name, the merged profile and project
settings, the *contents* of referenced files, resources and sbxm's version. A setting that doesn't reach a harness
(e.g. Claude-only settings for a Codex sandbox) isn't part of that harness's hash.

- `sbxm list` shows `changed` when the current config no longer matches, with the command to fix it.
- `sbxm open` refuses a changed sandbox instead of attaching to something built from an old config.
- `sbxm open <project> --harness h --rebuild` recreates it. The workspace is kept, but the agent's **session history in that
  sandbox is lost**. The old sandbox is removed only after the new kits validate.
- `sbxm config show <project> --harness <h>` prints exactly what's hashed, so you can see what changed.

## Security model

- Egress is deny-by-default and enforced by `sbx`'s proxy; only `network.allow` hosts (plus what the agent's kit
  needs) are reachable. `sbx policy log <sandbox>` shows what was allowed and blocked.
- Config only *names* secrets. Values stay in `sbx`, which injects them at the proxy.
- sbxm's metadata sits outside the mounted workspace, so an agent can't widen its own next sandbox.
- sbxm never changes `sbx`'s settings, secrets, skills store or policies. When one of them blocks something, sbxm
  tells you the command to run.
- Deleting files needs `rm --purge` plus a confirmation showing the exact paths; links and junctions are refused.

## Checking on a sandbox

**Which sandboxes exist and are they running?**

| Command | Shows |
|---|---|
| `sbxm list` | sbxm's sandboxes with status, config drift and orphans (`--json` for scripts). |
| `sbx ls` | Every sandbox `sbx` knows about, with agent, `running`/`stopped` and workspace. A sandbox that has gone from this list is finished and removed. |
| `sbxm task status` | Tasks: stage and status (`interrupted` if `sbxm` died), commits ahead of base, `result.md` and `review.md`. |

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
| `review-<round>.md`, `review.md` | a review ends | The reviewer's findings; `review.md` is the latest, starting with `Must-fix findings: <n>`. |

A review normally takes 6 to 12 minutes after its sandbox has started. The command's own output says which stage it is
in (gates, `review round 1`).

**Cleaning up.** Each sandbox that builds the project keeps its own `target\`, which is 3 to 4 GB. `sbxm rm <project>
--purge` removes every sandbox of a project and its workspace (asks first); `sbxm task rm --issue <n>` does it
for a task. Check `sbx ls` for stopped leftovers.

## Troubleshooting

- **Start with `sbxm doctor`.** Each failure says what's wrong and how to fix it.
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
a verified `git bundle` into its own repo (`<base_dir>\.sbxm\tasks\<id>\repo.git`). Only `sbxm` pushes. The table in
[Commands](#commands) lists every `task` command and flag.

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
the PR description (the task's `stopped` field says why, `rounds-exhausted` or `repeat-finding`); nothing is filed as
an issue unless you run `task file-findings`.

Every change to sbxm goes through a PR, including ones not made by workers, and gets the same review:
`task review --pr <n>` fetches the PR's head, runs the gates in the reviewer's sandbox, and posts the review as a
comment on the PR. There's no fix round; the author fixes the findings and runs it again. PRs from forks are
refused, because the code would run on your machine.

One-time setup:
- `just deploy-profiles` copies `profiles/sbxm-dev` into your `profiles_dir` (again after the profile changes). It
  installs Rust, a C toolchain with `pkg-config` and the OpenSSL headers (`openssl-sys` needs them), PowerShell 7,
  Pester 5 and `just`, allows crates.io, `cdn.pyke.io` (the ONNX Runtime download of the `fastembed` build),
  Microsoft's package host and the PowerShell Gallery, and needs the `anthropic` secret; a Codex reviewer also needs
  `openai` (`sbx secret ls`). A worker should not install software itself: what a build needs belongs in the
  profile's `setup.install`, or a clean sandbox (such as a PR review's) fails where the worker's passed.
- `sbxm task init` writes `sbxm-task.toml` in the repo's root: the worker, reviewer, profile, gates and time limits.
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
sbxm task run --issue 1         # start and review in one go, stopping before finish
```

Workers run for up to 2 hours and reviewers for 45 minutes by default (`--time-limit`, `--reviewer-time-limit` or
`sbxm-task.toml`). Each task keeps `task.json` (its stage, status, gate results, the sandbox names, and for an issue
task the fix round used so far, the round budget and why it stopped if it did), `gates.log`, `issue.md`, `result.md`,
`review-<round>.md` (`review.md` is the latest) and the transcripts under `<base_dir>\.sbxm\tasks\<id>\`; the clones
are `<base_dir>\tasks\<id>`, `<id>-review` and `<id>-gates`. A task killed halfway shows as `interrupted` in
`task status`; `task start --restart --issue <n>` (after the same confirmation as `task rm`) starts it again.

Sandbox setup takes about 3 minutes per task (apt, rustup, `cargo install just`). `base_dir` only moves the
workspace: each sandbox's own disk lives in the folder `sbx` keeps its state in, on the system drive, and `sbx` has no
setting to move it (a junction to a bigger drive was tried and failed for the `claude` agent; see [`docs/workflow.md`](docs/workflow.md)).
Budget about 7 GB for a fresh sandbox and much more once a large build runs in it: the disk image
is up to 20 GB for `/` plus a 10 GB Docker volume, and the debug builds of the gates (`check`, `clippy`, `test`
share one target folder) reached 17 GB for a crate that pulls in `fastembed`. A full disk shows up as a gate that
fails with exit 101 and no failing test, so keep over 10 GB free before starting a sandbox and run one at a time
unless 25 GB or more is free. `CARGO_PROFILE_DEV_DEBUG=0` in the `[gates]` commands drops debug info and should
shrink the builds (its effect on this repo's builds was not measured).
Deleting files inside a sandbox does not return space to the host; removing the sandbox (`task rm`) does.
`sbxm doctor` checks the base dir's free space, not the drive `sbx` uses.

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
