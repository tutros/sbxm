# Working with sbxm: workflow and use cases

This is the map. `README.md` is the reference for every command and flag (`sbxm <command> --help` shows the options);
this file shows how the commands fit together and when to use which. `sbxm` has three uses on top of a one-time setup.

## Setup (once)

1. `sbxm config init` writes `config.toml` and a `default` profile (refuses to overwrite).
2. Set `base_dir` in `config.toml` to a folder that is **not on `C:`**: on the Windows host this was built on, `sbx` fails
   to mount a workspace on `C:` (decision 56).
3. `sbxm doctor`: every line should start with `ok`.
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
sbxm task review --issue 63            # gates, independent review, fix rounds up to [worker] fix_rounds
sbxm task finish --issue 63            # push the branch and open the PR ("Fixes #63"); for an issue of
                                       # an open PR: push to that PR's branch and comment on the PR
sbxm task review --pr 69               # independent review of any PR of this repo, posted as a comment
sbxm task file-findings --pr 69        # dry run; add --create to file the findings as issues
sbxm task rm --issue 63                # after the merge: sandboxes, clones and record (asks first)
sbxm task run --issue 63               # start and review in one go; stops before finish
```

- **Stages:** `prepared`, `working`, `gating`, `reviewing`, `fixing`, `ready`, `finished`. A failed gate shows as
  `gates-failed`; a killed run shows as `interrupted` in `task status`, and `task resume --issue N` continues a
  failed or interrupted task from its stage, keeping its clone and commits.
- **Trust boundary:** the worker has no GitHub access and the host never runs git inside an agent's folder. Commits are
  collected through a verified `git bundle` into a host-owned repo, and only `sbxm` pushes.
- **Independence:** a worker never reviews its own change; the reviewer's harness differs from the worker's by default.
- **Limits to know:**
  - A `ready` task cannot be reviewed again with `task review --issue`; one that stopped (`stopped` in `task.json`)
    gets more fix rounds with `task resume --issue N --rounds N`, and otherwise the only redo is
    `task start --restart`.
  - `task review --pr` has no fix round (a PR task's `fix_rounds` is always 0): the author fixes the findings and
    reviews again after `task rm --pr N`.
  - `[worker] fix_rounds` (default 3) bounds the review-fix-review loop; a gate failure spends a round too. Rounds
    after the first review only the commits made since the last one, and one more review of everything confirms a
    clean one before the task is `ready`. If the budget runs out with findings still open, the task is still
    `ready`, with `stopped` set to `rounds-exhausted` in `task.json`, and `task finish` opens its PR as a draft whose
    body lists the reason and the must-fix findings left (issue 120); a clean task gets a normal PR. A task that
    continues an open PR puts the same list in its comment on that PR instead (it can't make the PR a draft).
  - `task start --spec FILE` makes a task from a file instead of a GitHub issue (id `spec-<name>-<hash>`,
    `source.md` in place of `issue.md`); `task review --spec FILE` continues it through the same gates/review/fix
    loop. `review.md` is its review (no PR, no GitHub call anywhere). `task finish --spec FILE` delivers it through
    `[finish] sink` in `sbxm-task.toml`: `local` (the default) keeps the branch in the task's `repo.git` and prints
    the `git fetch` command; `push` pushes it to `origin` as a new branch, no PR (refused while the task stopped or has
    must-fix findings left, unless `--push-unresolved`). `task rm --spec FILE` refuses to delete a result you haven't
    fetched into the checkout yet, unless `--force`.
- **Continuing an open PR (decision 169):** a finding filed with `task file-findings --pr N` names its PR (`PR: #N`),
  so its task starts from the PR's branch head. `task finish` for that task pushes to the PR's branch (no new flag),
  opens no new PR and comments on the PR with the commits added and `Fixes #<issue>`. The push is a fast-forward
  only, never a force: it is refused, with the reason, if the PR's branch moved on GitHub since the task started (or
  was deleted); start the task again with `task start --restart --issue <n>`. If only the comment failed, the task
  stays `ready` and `finish` again retries just the comment. The task is then `finished`; the re-review is
  `task review --pr N` as before (after `task rm --pr N` if the PR was reviewed already).
  - `task gates` is refused once a task is `ready`.

## How a change to sbxm itself goes

1. Plan with the `sdlc-planning` skill: numbered decisions in `sdlc/decisions.md`, spikes for unknowns.
2. File the work as issues; run `task start` and `task review`, then `task finish`.
3. Run `task review --pr N` on the PR: this is the independent review. Fix what it finds, with a test first.
4. The user merges. Never push to `main`; every change, docs included, goes through a PR.
5. `task rm` the finished task, which also gives back the disk its sandbox used.

## Practical notes

- **Which config and which binary.** `task` commands only see the tasks of the config dir they were started with
  (`SBXM_CONFIG_DIR`). If you run long jobs, use a recorded build of `sbxm` (commit, `--locked`, hash) rather than
  whatever is on `PATH`, so a result can be traced to the code that produced it.
- **Disk.** `base_dir` only decides where the *workspace* lives. Each sandbox's own disk (about 20 GB for `/`, plus a
  Docker volume) lives in `sbx`'s state folder on the system drive, and `sbx` has no setting to move it. A debug build
  of a large dependency tree (fastembed, ONNX Runtime) can fill it: `/tmp/target` reached 17 GB. Keep over 10 GB free
  before starting a sandbox, run one sandbox at a time unless over 25 GB is free, and delete `/tmp/target` in a
  sandbox you keep. Deleting files inside a sandbox does not return space to the host; removing the sandbox does.
  A junction from `sbx`'s `state` folder to a bigger drive was tried (2026-10-03, `sbx` 0.46.0) and does not work for the
  `claude` agent: sandbox creation fails with `policybind: resolve policy source`. A plain `shell` sandbox worked, so a
  quick test with it is not enough. See gap G19 in `sdlc/evals-workflow-notes.md`.
- **Gates that fit the disk.** `CARGO_PROFILE_DEV_DEBUG=0` in the `[gates]` commands of `sbxm-task.toml` drops debug
  info and should shrink the builds (its effect on this repo's builds was not measured). A full disk shows up as exit 101 with no failing test, so have the gate command print `^error`
  and `No space` lines as well as `FAILED` and `panicked`.
- **Workers should not install software.** If a worker installs a package itself, its gates pass where a clean
  sandbox fails (`pkg-config` for `openssl-sys` was the case). Put what the build needs in the profile's
  `setup.install`. Pre-built templates that make this strict are planned as their own milestone.
- **Reading logs on Windows.** Files written by `sbxm` are UTF-8 without a BOM. Windows PowerShell 5.1 reads them as
  ANSI (`✓` shows as `âœ“`); use `Get-Content -Encoding utf8` or PowerShell 7.
