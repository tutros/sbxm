# Spec: milestone 2b, `sbxm task`

Part of the PRD/spec experiment [140]. What and why: `sdlc/prd-m2b.md`. This file is the exact behavior, written so that
each rule can become a test (implementation rule 2). Decisions are cited by number. Where the code must be checked
before relying on it, the item is listed under "Details to verify first" at the end.

## 1. Modules and reuse

New (all under `src/`, one concern each):

| Module | Job |
|---|---|
| `github/` (`mod.rs`, `gh.rs`, `fake.rs`) | `GitHubBackend` trait, `GhBackend` (shells out to `gh`), `FakeGitHub` [142] |
| `task/config.rs` | Load and validate `sbxm-task.toml` (unknown keys are errors) [155] |
| `task/record.rs` | `task.json`: read, write atomically, stage transitions [145][146][153] |
| `task/select.rs` | Pure issue-selection function [154] |
| `task/gates.rs` | Run the two gate tiers, `HostRunner` trait + fake [160] |
| `task/repo.rs` | The host-owned bare repo, bundle fetch, push, clean checkout [159] |
| `task/prompts.rs` + `prompts/*.md` | Prompt templates, embedded with `include_str!` [156] |
| `task/pipeline.rs` | start / review / fix / finish orchestration, one function per command |
| `commands/task_*.rs` | One file per subcommand, like `commands/run*.rs` |

Reused unchanged: `SandboxBackend`/`FakeBackend` (create, exec, remove), `headless::run` (blocks, returns answer,
transcript and usage), `Harness` (claude, codex, antigravity), `run::checks` (command execution inside a sandbox, with
the in-sandbox `timeout`), the two-root layout idea from `run::id`, `confirm::Confirm`, `git.rs`'s hardened calls.

New work inside existing code, to be done in the slice that needs it: generalize `run::kits::build` (it takes a
`RunConfig` today) into a function that takes a profile, a harness list and resources; add a host-owned-repo git runner
beside the hardened one in `git.rs` (section 6); a `HostRunner` trait for the host gates (section 7).

## 2. `sbxm-task.toml` (at the target repo's root) [155]

```toml
[worker]
harness = "claude"            # claude | codex | antigravity (default claude)
model = "claude-opus-5-5"     # optional; the harness's default when absent
time_limit = "2h"             # whole number + s|m|h (as in the run-config)

[reviewer]
harness = "codex"             # default: a harness different from the worker's [148]
model = "gpt-5.6-sol"         # optional
time_limit = "45m"

[sandbox]
profile = "sbxm-dev"          # required: a profile in profiles_dir; no hard-coded default
# cpus = 4 / memory = "8g"    # optional; the global [resources] otherwise

[gates]
sandbox = ["cargo fmt --check", "cargo clippy --all-targets -- -D warnings", "cargo test"]
host = []                     # optional Windows tier; empty or absent = off [160]
timeout = "20m"               # per command

[prompts]                     # optional overrides: paths inside the repo [156]
# worker = "prompts/worker.md"
# reviewer = "prompts/reviewer.md"
# fix = "prompts/fix.md"
```

Rules:
- Unknown keys or tables are errors, naming the file and the key; a missing `[sandbox] profile` is an error.
- `gates.sandbox` absent: with a `Cargo.toml` at the repo root the default above applies; otherwise the load fails with
  `no gates configured for this repo; list the commands in [gates] sandbox in sbxm-task.toml` [151]. An explicit empty
  list is allowed and means "no sandbox gates" (stated in `task gates --dry-run` output).
- A prompt override path must stay inside the repo and not be a link (same rule as instruction files, decision 62).
- Flags override the file; the file overrides code defaults. `--repo`, `--base` are flags only.
- Reviewer default: if `[reviewer] harness` is absent it is the first of (codex, claude, antigravity) that differs
  from the worker's. Same harness for both, set explicitly, warns once and continues [148].
- The `[worker]`/`[reviewer]` harness must have its provider secret stored (claude `anthropic`, codex `openai`,
  antigravity `google` [125][137]); checked before anything is created.

## 3. The task record: `<base>/.sbxm/tasks/<id>/task.json` [145][146][153]

`<id>` is `issue-<n>` or `pr-<n>`. Layout:

```
<base>/.sbxm/tasks/<id>/   task.json  issue.md  result.md  review.md  review-<round>.md  gates.log
                           repo.git/  transcripts/  fix-prompt.md  bundle/
<base>/tasks/<id>/                    the worker's clone (mounted)
<base>/tasks/<id>-review/             the reviewer's clone (mounted), removed after the review
<base>/tasks/<id>-gates/              the clean checkout for host gates, removed after
```

Sandbox names: `sbxm-task-<id>-<harness>` (worker), `sbxm-task-<id>-review-<harness>` (reviewer). Names are stored in
`task.json`, never re-derived [147].

```json
{
  "schema": 1,
  "id": "issue-41", "kind": "issue", "number": 41, "repo": "owner/name",
  "title": "...", "base": "main", "branch": "issue-41",
  "stage": "working", "status": "running",
  "stages": [{"stage": "prepared", "at": "2026-10-01T15:00:00Z"}],
  "process": {"pid": 1234, "started_at": "2026-10-01T15:00:00Z"},
  "worker": {"harness": "claude", "model": null, "sandbox": "sbxm-task-issue-41-claude",
             "workspace": "E:/.../tasks/issue-41", "run": {"status": "completed", "usage": {}, "duration_s": 0}},
  "reviewer": null,
  "fix_round": false,
  "gates": [{"phase": "after-worker", "tier": "sandbox", "command": "cargo test", "exit": 0, "passed": true}],
  "related": [], "hooks": {}, "sbxm_version": "0.x", "config_hash": "..."
}
```

Stages and statuses:

| `stage` | Entered when | `status` values while/after |
|---|---|---|
| `prepared` | repo.git, clone, `issue.md` and sandbox are ready | `running`, `failed` |
| `working` | the worker agent starts | `running`, `completed`, `timed-out`, `failed` |
| `gating` | gates run (after worker, after fix) | `running`, `passed`, `gates-failed` |
| `reviewing` | the reviewer starts | `running`, `completed`, `failed` |
| `fixing` | the one fix round starts | `running`, `completed`, `timed-out`, `failed` |
| `ready` | last review has no must-fix, or the round is used | `ok` |
| `finished` | `finish` opened the PR | `ok` (carries `pr`) |

Rules:
- Every stage change is written (temp file + rename) before the work starts; a write failure stops the command [153].
- `process` holds the pid and start time of the running `sbxm`. `status` reports `interrupted` for any `running`
  status whose process is gone (pid absent, or a different start time).
- `hooks` is a free-form object, untouched by M2b; later stages (plan, release, learn) attach there [139].
- `schema` is checked on read; a newer schema is refused with a hint to upgrade sbxm.
- The record never contains secret values; usage and transcripts follow the run-results rules (decision 128).

## 4. Commands [143][157][158]

All commands check every input and precondition before the first write or backend call, and say
`<problem>; <fix>` in one line (implementation conventions). `--repo` defaults to the `origin` of the current
checkout (a GitHub URL is required; anything else is an error). `--base` defaults to the repo's default branch.

| Command | Does | Notable errors |
|---|---|---|
| `task init [PATH]` | Writes a starter `sbxm-task.toml` (valid as written for a Rust repo), refuses to overwrite | exists: `not overwriting it` |
| `task start (--issue N... \| --workers N)` | Section 5.1 | issue not open; blocked; question; clash; task exists |
| `task status [--issue N \| --pr N] [--json]` | One line per task: stage, status, commits ahead of base, `result.md`/`review.md` present, `interrupted` | no such task |
| `task review (--issue N \| --pr N)` | Section 5.2 | gates fail; secret missing; fork PR |
| `task gates --issue N [--tier sandbox\|host\|all] [--dry-run]` | Runs the tiers on the task's current state; `--dry-run` prints the commands and where they would run, changes nothing | worker still running |
| `task file-findings (--issue N \| --pr N \| --file F) [--create]` | Files a review's findings as issues [134][149]; a dry run unless `--create` | file malformed; secret in a finding |
| `task finish --issue N` | Pushes `issue-N` from `repo.git`, opens the PR (base = task base) with `Fixes #N`, `result.md` and `review.md` in the body | not `ready`; PR exists |
| `task rm (--issue N \| --pr N) [--yes]` | Removes sandboxes, clones and the task folder; asks first, showing the exact paths | task running |
| `task run --issue N` | `start` then `review`; stops before `finish` [158] | those of both |

Flags shared by `start`, `review`, `run`: `--worker-harness`, `--worker-model`, `--reviewer-harness`,
`--reviewer-model`, `--time-limit`, `--reviewer-time-limit`, `--profile`, `--base`, `--repo`.
`start --restart` discards an existing task of that id after the same confirmation as `rm` [153].
`start` on an existing task that is not finished refuses: `task issue-41 exists (stage working); use task status, or --restart` [153].

Output: human text by default; `status --json` prints the records. Exit codes: 0 success; 1 any failure
(including `gates-failed` and a `ready` task with must-fix findings left, which is reported, not an error).

## 5. Pipelines

### 5.1 `task start --issue N` (blocking; `--workers N` runs picks in parallel threads [144])

1. **Check before acting:** load config; resolve repo/base; secrets stored for the harnesses; profile exists and its
   kits validate (`kits::build`); `gh` works; the issue is open and passes selection [154]; no task with this id.
2. **Prepare:** reserve the task folders; `repo.git` = bare clone of GitHub (user's git config allowed, section 6);
   clone the workspace from `repo.git` at `--base`, switch to `issue-N`; set a repo-level `user.name`/`user.email`
   from the host's global git config (the sandbox doesn't see it); write `issue.md` (`gh issue view`) and the worker
   prompt; create the sandbox (`backend.create`); stage `prepared`.
3. **Work:** stage `working`; `headless::run` with the worker prompt, `time_limit` enforced in the sandbox [114].
   The prompt tells the agent the sandbox gate commands, to commit after each green step, to end the last commit with
   `Fixes #N`, not to push, and to write `.sbxm-task/result.md`. The agent has no GitHub access [141].
4. **Collect:** a fixed command in the sandbox writes `git bundle create` of `issue-N` (relative to base) into the
   workspace; the host verifies and fetches it into `repo.git` [159]; `result.md` and the transcript are copied to the
   task folder. Uncommitted changes in the clone are reported as a note (the agent may have ended early, #16).
5. **Gate:** stage `gating`; run the tiers (section 7). On failure: status `gates-failed`, stop (the sandbox stays for
   inspection until `rm`).
6. Print the next step (`task review --issue N`). `start` itself never reviews.

### 5.2 `task review --issue N` and `--pr N`

- **Issue:** requires a worker that has finished (not running, not interrupted). Gates run first (5.1 step 5) if
  they haven't passed since the last change. Then the reviewer: a clone of `repo.git` at `issue-N` into
  `tasks/<id>-review/`, its own sandbox, prompt with the issue text, `time_limit`, `review.md` written by the
  reviewer; `Reviewer: <harness> (<model>)` and `Must-fix findings: <n>` lines parsed from it. If `n > 0` and no fix
  round was used: one fix round (the worker's sandbox, fix prompt with `review.md`), collect (5.1 step 4), gates, and
  one more review (`review-2.md`). Final stage `ready`. Reviewer sandbox and clone are removed afterwards, also on
  error.
- **PR:** `gh pr view` (open, same-repo only: `isCrossRepository` is refused [87]); `repo.git` = bare clone, fetch the
  PR head; the linked issues' text is the review context; gates run (clean checkout, section 7); the reviewer runs
  once; the review is posted as a PR comment; no fix round. Task id `pr-<n>`; `related` links an `issue-<n>` task if
  one exists [158].
- The reviewer is told it cannot change files, commit, push or file issues, and to follow the code-review skill from
  the clone (`.claude/skills/sdlc-code-review/SKILL.md` when present).

## 6. Git trust boundary [159]

- The agent's clone is never a cwd or `--git-dir` of a host git command. All host git runs against `repo.git`,
  `<base>/.sbxm/...` temp folders, or the clean checkouts that come from it.
- Bundle step: `git -C <ws> bundle create <ws>/.sbxm-task/branch.bundle issue-N ^origin/<base>` runs **in the
  sandbox** (an `exec` with a fixed argv). The host then runs `git bundle verify` and
  `git fetch <bundle> refs/heads/issue-N:refs/heads/issue-N` inside `repo.git`, with hooks disabled
  (`-c core.hooksPath=<empty dir>`), no recursive submodules, and a size cap (config constant, default 500 MB).
- Network operations on `repo.git` (clone from GitHub, fetch a PR head, push) use the user's git configuration and
  credentials, because `repo.git` is host-owned. A new runner in `git.rs` does this; the hardened runner stays for
  agent-controlled repositories (contestant diffs).
- Clean checkouts for host gates and the reviewer use `git clone`/`worktree` from `repo.git`; no agent file reaches
  them except through committed objects.
- Tests: a bundle whose branch contains a malicious `.git/config` equivalent (hooks, fsmonitor) cannot execute
  anything on the fetch path; mirrors the #41 tests.

## 7. Gates [160]

- Two lists in config: `sandbox` and `host`. Each command runs as `sh -c` (sandbox) or the platform shell (host) under
  `[gates] timeout`; sandbox gates reuse the in-sandbox timeout wrapper (`timeout -v --kill-after=...` and the
  `-v` marker for timeouts, decision 123).
- Order: sandbox tier in the worker's sandbox on its workspace (after the agent exits, so a stuck agent can't race
  it); then, if `host` is non-empty, a clean checkout of `issue-N` from `repo.git` into `tasks/<id>-gates/` and
  the host commands there through `HostRunner`. The first failing command stops the phase.
- Results go to `task.json` (`gates[]`) and `gates.log`; a failure is `gates-failed` [152].
- `HostRunner` is a trait (`run(cwd, command, timeout) -> Output`); tests use a fake. Host gates inherit the user's
  environment but not `GIT_*` variables.
- Documented risk: host gates run agent-written code on the host (decision 84); they are off unless listed, and only
  run after the sandbox tier passed. The README and `task init`'s starter file say so next to `host = []`.
- `task gates --dry-run` prints, per tier, the exact commands, the working directory and whether the tier is off.

## 8. Issue selection [154]

`select(open: &[Issue], in_progress: &[u32], explicit: Option<&[u32]>, workers: usize) -> Selection`, pure.
`Issue { number, title, labels, body }`. Dependencies: `**Depends on:** #a, #b`; relations: `**Related:** #c` (the
regexes of the script: a line starting with the bold field name; every `#<digits>` on it). Rules, in order, per
candidate sorted by (label rank, number); label rank: `must-fix` 0, `should-fix` 1, others 9:
1. not in the explicit list (when given) → not a candidate (explicit numbers that aren't open issues warn);
2. already has a task → skip `already has a task`;
3. label `question` → skip `a question, needs your answer first`;
4. any `Depends on` issue still open → skip `blocked by open #…`;
5. related (either direction) to an in-progress or already picked issue → skip `related to #…, which has a task`;
6. otherwise pick, until `workers` picks (not applied to an explicit list).
`Selection { picks, skips: Vec<(u32, Reason)> }`; every skip is printed with its reason.

## 9. `GitHubBackend` [142]

```rust
trait GitHubBackend: Send + Sync {
    fn whoami(&self) -> Result<String>;
    fn default_branch(&self, repo: &str) -> Result<String>;
    fn issues_open(&self, repo: &str) -> Result<Vec<Issue>>;          // gh issue list --json
    fn issue(&self, repo: &str, n: u32) -> Result<IssueText>;          // gh issue view
    fn pr(&self, repo: &str, n: u32) -> Result<PrInfo>;                // headRefName, isCrossRepository, state, title, body, closing issues
    fn pr_create(&self, repo: &str, req: &PrRequest) -> Result<String>; // returns the URL
    fn pr_comment(&self, repo: &str, n: u32, body: &str) -> Result<()>;
    fn issue_create(&self, repo: &str, req: &IssueRequest) -> Result<u32>;
    fn labels(&self, repo: &str) -> Result<Vec<String>>;
}
```
`FakeGitHub` records calls and is scripted per test. Parsers are tested against output captured from the real `gh`
(fixtures with the `gh` version noted), like `sbx`'s.

## 10. Prompts [156]

`prompts/worker.md`, `reviewer.md`, `fix.md` are ported from the script's text (`New-Prompt`, `New-ReviewPrompt`,
`New-FixPrompt`), with `{{name}}` placeholders: `issue`, `number`, `branch`, `base`, `repo`, `gates_sandbox`,
`gates_host`, `review_path`, `previous_review_path`. An unknown placeholder, or a placeholder with no value, is an
error naming the template. The rendered prompt is written to the task folder so a run can be audited.

## 11. File-findings [134][149]

`task file-findings` is `scripts/file-review-issues.ps1`'s behavior behind `GitHubBackend`: parse a `sdlc/reviews/*.md` or a
task's `review.md`, render the issue template with permalinks, refuse secrets and replace personal paths, check
repo/login/labels first, file in dependency order, skip a finding whose hidden marker already exists, rewrite the
file's `Issues:` line. Dry run by default. The PowerShell script's Pester fixtures become the golden inputs.

## 12. Security summary

No GitHub access in sandboxes [141]; secrets only in `sbx`; no secret value in `task.json`, logs or prompts; the host
never runs git in an agent-controlled repo [159]; host gates are opt-in and documented [160]; fork PRs are refused
[87]; `rm` and `--restart` confirm with exact paths shown and never touch anything outside the task's own folders;
timeouts are enforced inside the sandbox.

## 13. Parity table (S1)

| Script | `sbxm task` | Note |
|---|---|---|
| `start` | `start` | + harness, base, `--restart` |
| `status` | `status` | + `interrupted`, `--json` |
| `review -Issue` | `review --issue` | gates tiers; one fix round |
| `review -Pr` | `review --pr` | posts the comment |
| `finish` | `finish` | base-aware PR |
| `remove` | `rm` | confirms |
| `file-review-issues.ps1` | `file-findings` | dry run default |
| `deploy-profiles.ps1` | stays a script | profile deployment, not task work |
| (new) | `init`, `gates`, `run` | [157][158] |

## Details to verify first (before the slice that relies on them)

Checked 2026-10-01 [161]: (1) `HeadlessOpts.model` is a `String` and `--model` is always passed, so an absent model needs
a code change (slice 0); (2) Codex gets no `model_reasoning_effort=high` (slice 0); (3) `git bundle` in a sandbox,
verify and fetch on the host with hooks off: works (spike S10).

Still to check inside the slice that needs it:
1. `gh pr view --json` field set and `gh issue list --json labels,body` output: capture fixtures and note the `gh`
   version (slice 2).
2. The Pester tests in `scripts/tests/` for selection and `file-review-issues` are the source of golden cases
   (slices 3 and 11).
3. Bundle behavior when the agent rewrites history or deletes the base ref (slice 5 tests it).