# Milestone 3: sandboxes from a prebuilt template

Planned 2026-10-15 with the user (issue #211, decisions 199-216: the interview 199-206, the plan choices 207-210,
the PR 253 reviews' questions 211-216), from spike S11 (`sdlc/spikes/S11.md`).

## Goal

A profile can opt in to having its sandboxes created from a prebuilt `sbx` template instead of running its install
steps on every create. That cuts creation from about 68 s to about 7 s and, in strict mode, lets a sandbox run with
only a runtime allow list: empty for this repo with an unchanged `Cargo.lock`. sbxm builds templates only on the
user's command [201], owns their records and removal [205], and every command that creates a sandbox follows the
profile's opt-in [206]. Profiles without `[template]` work exactly as today [202].

In scope: `claude` and `codex` [204]; `pi` if spike S15 passes. Deferred: `antigravity` (refused with a clear
message), per-task egress for a worker that changes a lock file (gap G1), a `--no-template` flag [206]. Warm templates
work for any language whose warm-up fills caches outside the workspace [207]. Dropped: any Gemini work [204].

## Plan-level choices to confirm

- **P1. What `--warm <repo>` runs.** Confirmed as decision 207: the repo's own `[warm]` in `sbxm-task.toml`
  (`commands`, `lock_files`), or the built-in cargo steps for a repo with a `Cargo.toml`; any other repo refused.
- **P2. Config drift with templates.** Confirmed as decision 208: the drift check compares the same variant the
  sandbox was created with; a newly matching template only gives an `open --rebuild` hint.
- **P3. The template hash and tag.** Confirmed as decision 209: build part of the profile, harness and `agent_kit`,
  warm or not; lock hash and warm commands in the record only. Tag `sbxm-<profile>-<harness>:<hash12>-<UTC build
  time>` (decision 211: every build gets a fresh tag).
- **P4. Where the build happens.** Confirmed as decision 210: sandbox `sbxm-template-<profile>-<harness>`, workspace
  `<base>/.sbxm/templates/build/<profile>-<harness>/`, both removed after every build (leftovers first on a rerun);
  no agent runs in a build sandbox. A build lock with process id and start time keeps a second build from removing
  a live one (decision 212).

All four are confirmed. Still open before slice 0: nothing. Spike S15 (Pi) decides only whether Pi joins [204].

## Structure (dependency order)

| Module | Responsibility |
|---|---|
| `src/config.rs` | `[template]` in `profile.toml`: `strict` (default `false`), `allow` (required); unknown keys are errors [202]. |
| `src/kit.rs` | the baked variant of the `common` mixin: no `setup.install`, `allow` from `[template]`; the template hash (P3). |
| `src/template.rs` (new) | records in `<base>/.sbxm/templates/<profile>-<harness>.json` [206]; tag naming; `resolve(profile, harness, lock)` → `Use(tag)` / `Stale(reason)` / `Missing`; the per-harness support list [204]. |
| `src/backend/` | `CreateSpec` gains an optional template (`-t TAG --pull never`); trait gains `template_save`, `template_ls`, `template_rm`; `SbxBackend` parsers tested on captured `sbx template ls --json`; `FakeBackend` records them. |
| `src/commands/template_*.rs` | `template build`, `template ls`, `template rm` [206]. |
| `src/state.rs`, `src/task/record.rs` | an optional `template` field (tag) per sandbox; absent in old files means none. |
| callers | `new`, `open --rebuild`, `task` (worker, reviewer, fix), `run` (contestants, judge) go through one helper that resolves the template and applies [200]/[203]. |
| `src/commands/doctor.rs` | templates with size and age, total, missing images, stale hashes, age over 14 days [203](c), [205](c). |

## Interfaces

### Profile

```toml
[network]
allow = ["archive.ubuntu.com", "sh.rustup.rs", "..."]   # build allow list for an opted-in profile

[template]
strict = true   # [200]
allow = []      # runtime allow list, required [202]
```

### Template record (`<base>/.sbxm/templates/<profile>-<harness>.json`)

`tag`, `template_hash`, `warm` (bool), `lock_files` and their combined `lock_hash` (warm only) [207], `sbx_version`, `size_bytes`, `created_at`, plus
`previous` tags kept because they were in use [205](a).

### Commands

| Command | Behavior |
|---|---|
| `template build <profile> [--harness h] [--warm <repo>] [--ignore-disk]` | `--ignore-disk` bypasses only this invocation's free-space refusal [194]. Refused for an unsupported harness, a profile without `[template]`, an `sbx` older than 0.46.0 [213], under `min_free_disk` [194], or with `--warm` on a repo with neither `[warm]` nor a `Cargo.toml` [207]. Creates the build sandbox with the full install kit and the build allow list, runs P1 when warm, stops it, `sbx template save`s it under the new tag, removes the build sandbox and workspace, writes the record, then removes the previous tag unless it is in use [205](a). Every build saves under a fresh tag [211], never onto an existing one (save onto an existing tag replaces silently, S11 Q8). |
| `template ls` | one line per record: profile, harness, tag, warm, size, age, in-use count; flags a record whose image is gone. |
| `template rm <profile> [--harness h]` | refused while `state.json` or any `task.json` records the tag, naming them [205](b); else `sbx template rm --force` and the record goes. |
| any create | resolve: `Use` → `-t TAG --pull never` + baked kit; `Stale`/`Missing` (or `sbx` older than 0.46.0 [213]) → strict: refuse naming `template build [--warm]`; not strict: install kit + one warning [200], [203]. |

## Work order (vertical slices, each TDD, each ends with fmt, clippy, cargo test and a commit)

0. **Real-`sbx` check of `codex` with `-t`** (as S11 did for `claude`): sandbox starts, `codex exec` answers, kit
   allow list applies. Done when written up in `sdlc/decisions.md`. Spike S15 (Pi) runs as its own session.
1. **Profile `[template]`**: parse, `allow` required, unknown keys refused. Done: `tests/profile.rs` cases.
2. **Baked kit and template hash**: snapshot of the baked `common` mixin; hash changes with each build-part field and
   not with runtime-only fields; the Claude managed-settings step stays in the baked harness kit and out of the
   hash [215]. Done: `insta` snapshots, hash tests.
3. **Backend template calls**: `CreateSpec` template, `template_save`/`ls`/`rm` on the trait, fake and `sbx` parser
   from a captured fixture. Done: unit tests on args and parsing.
4. **`template build` (cold) and records**: through `FakeBackend`, incl. refusals, cleanup on failure, removal of an
   unused previous tag and keeping a used one, and a rebuild with an unchanged hash saving under a new tag while the
   in-use old tag stays [211]; the build lock [212]: a second build refused while the first process is alive, and
   an interrupted build's leftovers removed when it is gone; a low-disk build refused before any write without
   `--ignore-disk` and reaching sandbox creation with it [194]. Done: `tests/template_build.rs`.
5. **`template build --warm`, cargo defaults** [207]: the copy rules, the built-in cargo sequence, the lock hash in
   the record, the refusal of a repo with neither `Cargo.toml` nor `[warm]`. Done: tests on the exec log.
5b. **`[warm]` in `sbxm-task.toml`** [207]: `commands` and `lock_files` parsed (unknown keys refused), run in order
   under the in-sandbox timeout, a failing command saves nothing. Done: `tests/template_build.rs` cases.
6. **`new` / `open --rebuild` use templates**: resolve, strict refusal, non-strict warning, `state.json` field, P2's
   drift and hint; a project `sandbox.toml` with `[template]` refused, and one adding `setup.install` or build hosts
   refused (strict) or falling back with a warning (non-strict) [214]. Done: `tests/new.rs`, `tests/open.rs` cases.
7. **`template ls` and `template rm`** with the in-use check over `state.json` and every `task.json`. Done: tests.
8. **`sbxm task` sandboxes**: worker, reviewer, fix rounds, `task resume`'s rebuilt worker; `task.json` field; the lock
   check at the commit each sandbox checks out [216], with tests for a PR whose head changes a lock file (refused
   in strict mode before the reviewer sandbox exists, though the base still matches) and for a worker that changes
   one after start (G1, fails inside the sandbox). Done: `tests/task_*.rs` cases.
9. **`sbxm run`**: contestants and judge, the strict refusal in preflight before any write. Done: `tests/run_*.rs`.
10. **`doctor`**: templates, total size, missing image, stale hash, older than 14 days; the template minimum `sbx`
    version [213] checked only when a profile has `[template]` (tests for an older `sbx` with and without one).
    Done: `tests/doctor.rs`.
11. **Docs and real test**: README, `docs/workflow.md` (incl. `template build` among the commands the disk check and
    `--ignore-disk` cover, and which minimum `sbx` version applies to what [213]), `profiles/sbxm-dev` gets `[template]`; `tests/real_sbx.rs`
    builds a cold template and creates one sandbox from it. Done: the real test passes.
12. **End-to-end check** (below).

## End-to-end checklist (real `sbx`, C: with at least 20 GB free)

1. `sbxm template build sbxm-dev --warm <this repo>`: tag printed, record written, build sandbox gone.
2. `sbxm new` on a scratch project: under 15 s, no install step in the output.
3. In it, the default gates pass with `[template] allow = []`; `crates.io` answers 403.
4. Change a dependency in the scratch copy's `Cargo.lock`, `sbxm new` another project from it: strict refusal names
   `template build --warm`.
5. `sbxm task run --issue N` on a real issue: every sandbox records the tag in `task.json`.
6. `template rm sbxm-dev` while that task exists: refused, naming it. After `task rm`: removed.
7. `template build sbxm-dev --harness codex`, then a codex sandbox from it.
8. `doctor` lists the templates and their total size; `sbx ls`, `sbx template ls` match sbxm's records.

## Out of scope

Per-task egress (G1); `antigravity`; Gemini; exporting templates (`--output`, `template load`);
cloud templates (S14, issue #245); a rebuild schedule [201]; batching tasks in one sandbox [147].

## Risks

1. **`--kit` and templates are experimental in `sbx`.** A template-only minimum of 0.46.0 [213]; keep the real test.
2. **Disk on C:.** 1.3 to 2.6 GB per template; mitigated by [205] and the free-disk check [194].
3. **A warm template goes stale on every dependency change.** Strict sandboxes are refused until a rebuild; the
   message names the command. Measure how often this happens before automating anything.
4. **The agent version is frozen in a template.** `doctor` warns after 14 days [203](c).
5. **Timings under contention are unmeasured** (S11 risk 8): log real task times before and after.
