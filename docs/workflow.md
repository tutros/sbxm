# Working with sbxm: workflow and use cases

This is the map. `README.md` is the reference for every command and flag (`sbxm <command> --help` shows the options);
this file shows how the commands fit together and when to use which. `sbxm` has three uses on top of a one-time setup.

## Setup (once)

1. `sbxm config init` writes `config.toml` and a `default` profile (refuses to overwrite).
2. Set `base_dir` in `config.toml` to a folder that is **not on `C:`**: on the Windows host this was built on, `sbx` fails
   to mount a workspace on `C:` (decision 56).
3. `sbxm doctor`: every line should start with `ok`. A `skip` line says what to set for a check to run (on macOS and
   Linux, `sandbox_state_dir` for the sandbox drive), and a `warn` line is a problem that doesn't fail `doctor` yet
   (the sandbox drive under `min_free_disk`).
4. Store the provider secrets in `sbx` (`sbx secret ls`): `anthropic` for Claude, `openai` for Codex (also the default
   reviewer for a Claude worker), `google` for Antigravity. For `sbxm task`, run `gh auth setup-git` once.
5. `just deploy-profiles` copies the repo's `profiles/*` into the folder `sbxm config profiles-dir` reports. It replaces
   whole profiles, so deploy from an up-to-date `main`: a profile change that is still on an open PR is lost if you
   deploy from another branch.

## Use case 1: a sandbox for a project

```
sbxm new demo --harness claude [--profile p] [--seed dir]
sbxm open demo --harness claude      # attach; starts or creates the sandbox
sbxm list                            # status, config current/changed, orphans
sbxm stop demo --harness claude
sbxm rm demo --harness claude        # keeps the workspace; --purge deletes it (asks first)
```

The config has three layers: global `config.toml`, then a profile, then the project's `sandbox.toml`. A hash of the
merged result is recorded with the sandbox. If the config changed, `open` refuses until you run `open --rebuild`.
`sbxm config show` previews exactly what would be built, and `sbxm doctor` checks the setup.

### Faster sandboxes from a template (optional, per profile)

```
sbxm template build sbxm-dev --warm .   # once, and again when it goes stale
sbxm new demo --profile sbxm-dev        # nothing is installed (about 7 s instead of 68 s in spike S11)
sbxm template ls                        # tag, size, age, users
sbxm template rm sbxm-dev               # refused while a sandbox or task uses it
```

A profile opts in with `[template]` (README, *Templates*). From then on every sandbox of that profile — `new`, `open`,
every `sbxm task` sandbox, `sbxm run`'s contestants and judge — starts from the template and gets only
`[template].allow`. When the template is stale or missing, a `strict` profile refuses and names the command to run;
the usual one is `sbxm template build <profile> --warm <repo>` after a lock file changed. A non-strict profile warns
and installs as before. `sbxm doctor` lists the templates and warns about stale ones, missing images and templates
over 14 days old (the agent inside is frozen at the version it was saved with; rebuild to update it).

Which minimum `sbx` version applies: `min_sbx_version` in `config.toml` (default 0.43.0) for everything, and 0.46.0
built into sbxm for templates. `doctor` fails on the template minimum only when a profile has `[template]`;
`template build` refuses below it; a create treats an older `sbx` like a missing template.

## Use case 2: compare agents on one task

```
sbxm run init run.toml               # starter run-config: Claude against Codex
sbxm run run.toml                    # one throwaway sandbox per contestant and repeat
sbxm run show <run-id> [--diff]      # read a saved run
```

Each contestant (harness, model, optional profile) gets a fresh copy of an optional seed folder. The answer, the diff
and the transcript are saved under `<base_dir>/.sbxm/runs/<run-id>/`. Evaluation is optional and combinable:
executable checks (`[[eval.checks]]`, exit 0 passes), an LLM judge with a rubric that sees blind labels (A, B, ...), and
cosine similarity between text answers (`[eval.cosine]`, which loads the model from local files named by `model_dir`;
decision 166). A ranking is computed in code from the judge's scores.

## Use case 3: GitHub issues to pull requests (`sbxm task`)

```
sbxm task init                         # writes sbxm-task.toml: worker, reviewer, profile, gates
sbxm task start --issue 63             # worker in its own sandbox; commits collected; gates run
sbxm task status                       # stage, status, commits ahead, result/review present
sbxm status --watch                    # a live dashboard over every task (decision 171, part 4 of 6):
                                       # stage, status, time in stage, commits ahead, must-fix findings
                                       # left, PR link, last run.log line; redraws every 2s on a terminal
sbxm task review --issue 63            # gates, independent review, fix rounds up to [worker] fix_rounds
sbxm task finish --issue 63            # push the branch and open the PR ("Fixes #63"), and post the
                                       # in-task review on it as its review; for an issue of an open
                                       # PR: push to that PR's branch and comment on the PR
sbxm task review --pr 69               # independent review of any PR of this repo, posted as a comment
                                       # (refused for a head the in-task review covered; --force)
sbxm task file-findings --pr 69        # dry run; add --create to file the findings as issues
sbxm task rm --issue 63                # after the merge: sandboxes, clones and record (asks first)
sbxm task rm --merged --dry-run        # list every task whose PR (or spec branch) is merged
sbxm task rm --merged                  # ...then remove them all in one go, archiving first
sbxm task run --issue 63               # start and review in one go; stops before finish
sbxm task run --spec idea.md           # the same for a spec task; no GitHub call anywhere
sbxm task run --issue 210 --issue 202 --spec sdlc/specs/x.md   # a list, one after another
```

- **Stages:** `prepared`, `working`, `gating`, `reviewing`, `fixing`, `ready`, `finished`. A failed gate shows as
  `gates-failed`; a killed run shows as `interrupted` in `task status`, and `task resume --issue N` continues a
  failed or interrupted task from its stage, keeping its clone and commits.
- **Trust boundary:** the worker has no GitHub access and the host never runs git inside an agent's folder. Commits are
  collected through a verified `git bundle` into a host-owned repo, onto a separate ref for the agent's own,
  unrewritten commits; before gates, review or finish see them, `sbxm` rewrites every one of those commits onto the
  delivered branch with any closing reference or mention neutralised, rebuilding them unsigned; a commit over 1 MiB
  (header plus message) is refused instead (decision 198). Only `sbxm` pushes.
- **Independence:** a worker never reviews its own change; the reviewer's harness differs from the worker's by default.
- **Limits to know:**
  - A `ready` task cannot be reviewed again with `task review --issue`; one that stopped (`stopped` in `task.json`)
    gets more fix rounds with `task resume --issue N --rounds N`, and otherwise the only redo is
    `task start --restart`.
  - `task review --pr` has no fix round (a PR task's `fix_rounds` is always 0): the author fixes the findings and
    runs `task review --pr N` again, which reviews the new head in place, keeping the earlier rounds (decision 190;
    an unchanged head is refused unless `--force`).
  - **The in-task review counts as the PR's review (decision 187):** `task finish --issue N` posts the review
    recorded in `task.json` on the PR it opens (risk section first, the same one the description opens with, then a
    line naming the reviewed commit), a draft's failing review included, when that review covered the PR's head.
    `task review --pr N` then refuses that head before anything is written (`already reviewed at <commit>`;
    `--force` reviews it anyway); a later commit on the PR (a hand fix, a rebase) is reviewed as usual. Only the
    issue task's own `task.json` on this machine decides, never a comment on GitHub; with no task folder recording
    the PR, the review runs as before. A task that continued an open PR and spec tasks (which open no PR) are
    unchanged.
  - `[worker] fix_rounds` (default 3) bounds the review-fix-review loop; a gate that ran and failed spends a round too
    (one that could not run at all stops the task without spending any). Rounds
    after the first review only the commits made since the last one, and one more review of everything confirms a
    clean one before the task is `ready`. If the budget runs out with findings still open, the task is still
    `ready`, with `stopped` set to `rounds-exhausted` in `task.json`, and `task finish` opens its PR as a draft whose
    body lists the reason and the must-fix findings left (issue 120); a clean task gets a normal PR. A task that
    continues an open PR puts the same list in its comment on that PR instead (it can't make the PR a draft).
  - `task start --spec FILE` makes a task from a file instead of a GitHub issue (id `spec-<name>-<hash>`,
    `source.md` in place of `issue.md`); `task review --spec FILE` continues it through the same gates/review/fix
    loop, and `task run --spec FILE` does both in one go, stopping before `finish` (with `--restart`, the spec is
    read again after you confirm: a valid edit at the same path is used, and a file that disappeared, became
    unusable or now names another task fails the start after the old task is gone; decision 184).
    `review.md` is its review (no PR, no GitHub call anywhere). `task finish --spec FILE` delivers it through
    `[finish] sink` in `sbxm-task.toml`: `local` (the default) keeps the branch in the task's `repo.git` and prints
    the `git fetch` command; `push` pushes it to `origin` as a new branch, no PR (refused while the task stopped or has
    must-fix findings left, unless `--push-unresolved`). `task rm --spec FILE` refuses to delete a result you haven't
    fetched into the checkout yet, unless `--force`.
  - **A list of tasks, one command (decision 193, issue 201):** `--issue`/`--spec` are repeatable and may be
    combined (`--issue 210 --issue 202 --spec sdlc/specs/x.md`), run in the order written; the other flags apply to
    every task of the list, and there is no list file. Every target's own selection problem is checked before the
    first target's sandbox is created (issue 201 review M-1, repeat of M-1): for an issue, the same rules an
    explicit `task start --issue A --issue B` already applies — a question, blocked by an open `Depends on` issue,
    related to another issue in the same batch or an existing task, doesn't exist, isn't open, already has a task,
    or names a PR that can't be continued; for a spec, a file that can't be read or already has a task. A later
    target's problem is refused up front this way instead of only once an earlier, valid target has already run;
    skipped, not an error, when the list's shared repo can't even be resolved (no `--repo` and no git origin), in
    which case each target is checked the old way, on its own turn. The tasks are independent: a later one never
    gets an earlier one's work. A task that stops short of `ready` (must-fix findings left, a failed worker, a gate
    that could not run) is recorded and the list goes on to the next one; the list itself stops only for a problem
    that would hit every task (`sbx` not reachable — the preflight's own call to it, or `SandboxBackend::exec`
    itself failing while a worker, fix round or reviewer ran, issue 201 review M-2, repeat of M-2 — a missing
    secret, or too little free disk — checked again before each task, not only the first, since a task's own
    sandbox can fill the drive the next one needs). It prints a
    summary line per task at the end (stage, must-fix findings left, the next command). Exit 0 only when every task
    of the list ended `ready`; 1 when at least one failed or was never started because the list stopped. `--restart`
    is refused with more than one task in the list: there is nothing left to show the user when several tasks' old
    copies would be discarded at once.
- **Continuing an open PR (decision 169):** a finding filed with `task file-findings --pr N` names its PR (`PR: #N`),
  so its task starts from the PR's branch head. `task finish` for that task pushes to the PR's branch (no new flag),
  opens no new PR and comments on the PR with the commits added and `Fixes #<issue>`. The push is a fast-forward
  only, never a force: it is refused, with the reason, if the PR's branch moved on GitHub since the task started (or
  was deleted); start the task again with `task start --restart --issue <n>`. If only the comment failed, the task
  stays `ready` and `finish` again retries just the comment. The task is then `finished`; the re-review is
  `task review --pr N` as before (reviewed again in place if the PR was reviewed already).
  - `task gates` is refused once a task is `ready`.
- **Cleanup in one command (decision 195, issue 202):** `task rm --merged` finds every task whose work is merged
  (an issue's task or a PR-review task whose PR merged on GitHub, a spec task whose branch is reachable from its
  base after a fetch) and removes it after one confirmation, instead of a `task rm` typed by hand per task. A task
  whose PR is still open or was closed without merging is skipped without a word. Every other task left alone is
  named with the reason, `--yes` or not: a spec task's merge can't be told from "not yet merged" (a squash or rebase
  merge isn't detected), its result was never fetched out of its own `repo.git` (decision 178's protection, which
  still applies), or its task folder, its `task.json` or a spec task's `repo.git` is a link, or its `task.json` names another task than its folder (never followed, nothing behind it read; a linked `.sbxm/tasks`
  refuses the whole command). For a spec task with
  the `local` sink, the worktree and the local branch in your own checkout are never deleted either; the dry run and
  the removal summary both name the two `git` commands that do, instead of running them. `--dry-run` only
  lists; it asks and removes nothing. Each task's text records (`review.md`, every `review-<n>.md`, `result.md`,
  `run.log`, `gates.log`, `task.json`) are archived to `.sbxm/task-archive/<id>/` before its folder goes (a rerun after a failed removal reuses an identical archive; one that differs is refused), and the output names the disk
  given back.

### The same tasks in Docker Cloud Sandboxes (milestone 4)

```
sbxm task run --spec idea.md --cloud   # this task's sandboxes in the cloud; the task remembers it
sbxm task review --pr 69 --local       # this review here, whatever [sandbox] location says
sbxm status                            # the tasks, then the running cloud sandboxes and their time left
```

`[sandbox] location = "cloud"` in `sbxm-task.toml` makes the cloud a repo's default (`"local"`, or no key, keeps
it here); `--cloud`/`--local` on `task start`, `task run` and `task review --pr N` override it for that command.
After that, `task.json`'s `location` decides for every command of the task. The phases are the same as locally;
what differs is how the work gets in and out (bundles copied by `sbx --cloud cp`, a clone under `$HOME/work/<id>`),
how long each sandbox lives, and what is refused up front. README, *Running tasks in Docker Cloud Sandboxes*, has
every detail.

| Command | In the cloud |
|---|---|
| `task start`, `task run` | Location from the flag, else the key. The preflight below, then the worker sandbox (the profile's kits, a TTL, `--on-timeout delete`, the strict denies before the agent), bundle and prompt in, worker, sandbox gates, bundle out, verify, fetch, host gates. The worker sandbox goes when the task stops. |
| `task review --issue/--spec` | The recorded location. A reviewer sandbox per round; a fix round in the worker sandbox, made again from a bundle of the branch if it is gone. |
| `task review --pr N` | Location as for `start`; the PR's head goes in as a bundle from `repo.git`. |
| `task resume` | Always in the cloud: the preflight again, then a gone worker sandbox made again from the recorded profile and a bundle of the branch. |
| `task gates` | In the recorded location; a gone worker sandbox is made again first, and removed when the gates end. |
| `task finish` | Host-side only; then removes any cloud sandbox of the task still open (one whose removal at the stop failed), retried. |
| `task rm`, `task rm --merged` | Remove the task's open cloud sandboxes (retried) and their scoped secrets, then the folder; a sandbox already gone counts as removed, and one that may still run keeps the folder. `--merged` archives `task.json` and `run.log`, with the lifetimes, first. |
| `task status`, `status` | `task status` shows the recorded location; `status` adds the running `sbxm-` cloud sandboxes, or one line when the cloud can't be read (still exit 0). |
| `doctor` | Lists the running cloud sandboxes (a warning when the cloud can't be read), warns about an `sbx` older than 0.46.0 while the cloud is in use, and about `/tmp` in the gates of a cloud repo (the current folder's). |

**Refused before anything is written,** each message ending "or run it here with `--local`" (decision 261): `sbx`
older than 0.46.0 (upgrade it); the cloud not answering `sbx --cloud ls` (`sbx login`); a harness whose provider
login the cloud can't store (only `claude` and `codex` run there); a profile secret other than the `anthropic` and
`openai` logins, which has no value source in the cloud yet (take it out of the profile); a provider login the task
needs missing in the cloud, the harness's own or one the profile names (`sbx secret set <provider> --oauth
--cloud`). sbxm never falls back to a local sandbox. The strict check can't be made up front: it reads the
sandbox's own policy, so it runs after `create` and before any agent, and removes the sandbox when it fails.

- **Build under `$HOME`, never `/tmp` (decision 257).** In a cloud sandbox `/tmp` is a tmpfs of half the memory,
  and a cargo target folder filled it (spike S14, Q5). There is one gate list for both locations, so change your
  own `sbxm-task.toml` (it isn't in this repo) to build under `$HOME`:
  ```toml
  [gates]
  sandbox = [
    "cargo fmt --check",
    "CARGO_TARGET_DIR=$HOME/target cargo clippy --all-targets -- -D warnings",   # was /tmp/target
    "CARGO_TARGET_DIR=$HOME/target cargo test",
  ]
  ```
  `sbxm doctor`, run in a repo whose `sbxm-task.toml` says `location = "cloud"`, warns about each sandbox gate that
  still names `/tmp`.
- **How long a cloud sandbox lives (decisions 259, 263).** Each is created with `--ttl` and `--on-timeout delete`.
  The worker sandbox's TTL is the worker's time limit + every sandbox gate's timeout + `fix_rounds` x (the reviewer's
  time limit + the worker's + every sandbox gate's timeout) + 30 minutes (14h45m with the defaults and three gates),
  `fix_rounds` being the task's own budget (recorded at its start, plus any `task resume --rounds N`):
  it lives through the task's automatic run, so fix rounds keep a warm build, and is removed when the task stops,
  `ready` or for any other reason, so nothing is billed while the task waits for you. `task resume`, `task gates` or
  a later review's fix round makes it again from a bundle of the branch when needed. A reviewer sandbox lives for one
  round, with the reviewer's time limit + 30 minutes. A removal that fails with "temporarily unavailable" is retried
  after 5, 10 and 20 s; the TTL is the backstop, and `sbxm status` and `sbxm doctor` list what is still running.
  Every sandbox's lifetime is in `task.json`'s `sandboxes` and in `run.log`, so a retro can add up the hours (read
  them before a plain `task rm`, which deletes them; `task rm --merged` archives them).
- **One task at a time, for now (decision 258).** Milestone 4 adds no parallel orchestration. The experiment still
  to run after the merge: 2, then 3, cloud tasks at once as separate background `task run`s, measuring tasks per
  hour, cost and failures; a `--parallel` option is built only if it justifies one. `task start --workers N` works
  in the cloud as it does here.
- **Troubleshooting a strict cloud sandbox (decision 264).** sbxm *removes* hosts from a strict profile's cloud
  sandbox: every host outside the harness's provider hosts and `[template].allow` is denied, and the list goes to the
  task's `run.log` (`strict cloud sandbox <name>: sbxm denied <hosts>`). When something fails unexpectedly in a strict
  cloud sandbox (the agent or a gate; the install steps ran before the denies), first check whether one of the hosts
  listed as denied there is actually needed; if it is, add it to `[template].allow`. A denied host can fail at DNS,
  with connection refused or with a proxy 403 (S14, Q1), so don't look only for a 403.

## How a change to sbxm itself goes

1. Plan with the `sdlc-planning` skill: numbered decisions in `sdlc/decisions.md`, spikes for unknowns.
2. File the work as issues; run `task start` and `task review`, then `task finish`.
3. The independent review: for a PR `task finish` opened, the in-task review it posted is that review (decision
   187); run `task review --pr N` after any later commit on the PR, and on a PR made some other way. Fix what it
   finds, with a test first.
4. A low-risk PR (the reviewer's level and the path-rule floor) whose latest review covered its head commit with
   0 must-fix is merged by GitHub's auto-merge, armed with `gh pr merge <N> --auto --merge --match-head-commit <sha>`
   for that reviewed commit; before any later push, disarm it (`gh pr merge <N> --disable-auto`) and review the new
   head before arming it again (decision 266, an experiment). The user merges a medium- or high-risk PR. Never push
   to `main`; every change, docs included, goes through a PR.
5. `task rm` the finished task, which also gives back the disk its sandbox used.

Work in a sandbox needs no approval of single commands; work an agent session does on the host asks for each one.
So keep the host side thin: fix review findings with `task resume --rounds N` instead of by hand, and run docs and
state changes as spec tasks. What has to stay on the host is what calls `sbx` or publishes: the `sbxm task`
commands themselves, pushes, PRs, issues and profile deploys. The repo's `.claude/settings.json` allows these
commands without a prompt, for the Bash and the PowerShell tool: `gh issue view`, `gh issue list`, `gh pr view`,
`gh pr list`, `gh pr diff`, `gh pr checks`, `sbx ls` and `sbx template ls`. No `git` command is in the list: `git
log`, `git diff` and `git show` can write a file with `--output=<file>`, `git status` refreshes the index file, and
a rule cannot exclude an option (Claude Code already runs the read-only forms of `git` in Bash without asking).
Anything that publishes, deletes, writes or starts a sandbox still asks.

The `sbxm` commands that only report (`status`, `task states`, `task logs`, `list`, `config show`,
`config profiles-dir`, `run show`) are not in that file (decision 192): a recorded build's file name changes with
every build and each call sets `SBXM_CONFIG_DIR` first, so a rule would need a wildcard in the middle of the command. Allow them on your own
machine instead: a small wrapper script at a fixed path that sets the config dir and calls the current recorded
build, rewritten by whatever makes that build, and one allow rule per report command for that path in
`.claude/settings.local.json`, which is not checked in because it names a path of one machine. Claude Code refuses
to let an agent session edit its own permission settings, so you add the rules yourself. Whoever can edit the
wrapper decides what the allowed command does: keep it outside any folder an agent's sandbox mounts. Two commands
that look like reports are left out on purpose and keep asking, because they write: `doctor` (its writability
check creates and removes a file in the base dir, and its kit check writes and removes kits in the temp dir) and
`task status` (it appends its output to the `run.log` of each task it shows, decision 170; `sbxm status` shows the
same tasks and only reads).

## Practical notes

- **Which config and which binary.** `task` commands only see the tasks of the config dir they were started with
  (`SBXM_CONFIG_DIR`). If you run long jobs, use a recorded build of `sbxm` (commit, `--locked`, hash) rather than
  whatever is on `PATH`, so a result can be traced to the code that produced it.
- **Disk.** `base_dir` only decides where the *workspace* lives. Each sandbox's own disk (about 20 GB for `/`, plus a
  Docker volume) lives in `sbx`'s state folder on the system drive, and `sbx` has no setting to move it. A debug build
  of a large dependency tree (fastembed, ONNX Runtime) can fill it: `/tmp/target` reached 17 GB. Keep over 10 GB free
  before starting a sandbox, run one sandbox at a time unless over 25 GB is free, and delete `/tmp/target` in a
  sandbox you keep. Deleting files inside a sandbox does not return space to the host; removing the sandbox does.
  sbxm checks this for you: `doctor` reports that drive's free space, and `new`, `open` (when it creates or rebuilds),
  `run`, `template build`, `task start`, `task review`, `task run` and `task resume` refuse before writing anything when it is under
  `min_free_disk` in `config.toml` (default `"10g"`; `"0"` switches the check off), naming the drive and its free
  space; `--ignore-disk` lets one command through. A `task run` list (decision 193) checks again before every task,
  not only the first, since a task's own sandbox can fill the drive the next one needs; a failure there stops the
  rest of the list. A cloud task skips the check (its sandboxes don't use that drive). The folder is `%LOCALAPPDATA%\DockerSandboxes` on Windows, or
  `sandbox_state_dir` in `config.toml` on any platform; on macOS and Linux without that key nothing is checked (a full
  drive makes `sbx` answer `try: sbx login`, which looks like a login problem).
  A junction from `sbx`'s `state` folder to a bigger drive was tried (2026-10-03, `sbx` 0.46.0) and does not work for the
  `claude` agent: sandbox creation fails with `policybind: resolve policy source`. A plain `shell` sandbox worked, so a
  quick test with it is not enough. See gap G19 in `sdlc/evals-workflow-notes.md`.
- **Gates that fit the disk.** `CARGO_PROFILE_DEV_DEBUG=0` in the `[gates]` commands of `sbxm-task.toml` drops debug
  info and should shrink the builds (its effect on this repo's builds was not measured). A full disk shows up as exit 101 with no failing test, so have the gate command print `^error`
  and `No space` lines as well as `FAILED` and `panicked`.
- **Workers should not install software.** If a worker installs a package itself, its gates pass where a clean
  sandbox fails (`pkg-config` for `openssl-sys` was the case). Put what the build needs in the profile's
  `setup.install`. A strict `[template]` profile makes this hold: its sandboxes reach only `[template].allow`, so a
  worker can't install what the template doesn't have. A worker that changes `Cargo.lock` then fails its gates inside
  the sandbox (gap G1, per-task egress).
- **Templates take disk too.** Each is 1 to 3 GB on the same drive as the sandboxes. A build removes the template it
  replaces unless a sandbox or task still records it; `sbxm template ls` shows who does.
- **Tests after a hand fix.** A hand fix on a task's branch was not gated in a sandbox, so run the checks on the
  host: `cargo fmt --check`, clippy, and the tests the change touches (`cargo test --test <file>`, plus the unit
  tests of a changed module). The full `cargo test` takes about 12 minutes on Windows; run it when the change is
  Windows-specific (paths, file locks, process handling), when it makes a command fail where it used to succeed (then
  grep every test that calls the command first), or when no sandbox gate will run on the result. The PR review's
  sandbox gates run the full suite on Linux in either case.
- **Reading logs on Windows.** Files written by `sbxm` are UTF-8 without a BOM. Windows PowerShell 5.1 reads them as
  ANSI (`✓` shows as `âœ“`); use `Get-Content -Encoding utf8` or PowerShell 7.
