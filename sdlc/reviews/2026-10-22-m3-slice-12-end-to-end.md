# Slice 12 (M3): end-to-end check of templates (2026-10-21/22)

Run from `m3-implementation`, release binaries built by `H:\sbxm-e2e\build-known.ps1`: `77b6a10` (slices 0-11) for
steps 1-4 and 7, `537ecf6` (+ M-1's fix) for the rerun of step 1 and steps 5-6, `17e1e0e` (+ M-2's fix) to confirm
step 8. `sbx` 0.46.0, secrets `anthropic` and `openai` stored. A separate `SBXM_CONFIG_DIR` (`H:\sbxm-e2e-m3\config`,
base `H:\sbxm-e2e-m3\base`, its own copy of the repo's `profiles/`), so the strict `sbxm-dev` profile never reached
the user's normal config. C: had 22-31 GB free. Real agent runs: Claude worker, Codex reviewer, one `codex exec`.

| Checklist item | Result | Evidence |
|---|---|---|
| 1. `template build sbxm-dev --warm <this repo>` | Pass, with S-1 | Tag `sbxm-sbxm-dev-claude:4b91d11b30d8-20261021t2150` printed, record written (`warm: true`, `Cargo.lock`, 3.0 GiB), build sandbox gone; 283 s. Rebuilt after M-1 in 311 s; the unused previous tag was removed |
| 2. `sbxm new` on a scratch project: under 15 s, no install step | Pass | 13.8 s from a `git archive` copy of the repo; image = the tag; only the Claude kit's own three steps ran, no apt or rustup |
| 3. Default gates pass with `[template] allow = []`; `crates.io` 403 | Pass | `crates.io` and `index.crates.io` 403; `cargo fmt --check`, clippy (only sbxm itself compiled, 21 s) and the full `cargo test` passed, 144 s in total |
| 4. Changed `Cargo.lock` in a copy: strict refusal names `template build --warm` | Pass | `the template of profile 'sbxm-dev' for claude is stale: the lock files of ... (Cargo.lock) differ ...; rebuild it with sbxm template build sbxm-dev --warm <repo>`; no project folder or metadata written |
| 5. `task run --issue N`: every sandbox records the tag in `task.json` | Pass after M-1 | First run refused (M-1). After the fix, issue #36: worker 1 commit, 3 sandbox gates passed, Codex review 0 must-fix, `ready` in 325 s; `task.json` has the claude tag on the worker and the codex tag on the reviewer. No fix round ran. `task finish` opened PR #268 |
| 6. `template rm` while the task exists: refused, naming it; after `task rm`: removed | Pass | `template ... is in use by task issue-36; remove those first`; after `task rm --issue 36 --yes`: `removed template ...`, gone from `sbx template ls` |
| 7. `template build --harness codex`, a strict codex sandbox: `codex exec` answers, `github.com` 403 | Pass | Cold build 140 s (run before step 5: the task's Codex reviewer needs it under a strict profile); sandbox in 13.4 s; `codex exec` answered `PONG`; `github.com` 403 |
| 8. `doctor` lists templates and total size; `sbx ls`, `sbx template ls` match | Pass after M-2 | `doctor`: one line per template, `1 template, 2.1 GiB in total`, matching `sbx template ls` and `sbx ls`. `sbxm list` showed the template sandbox as changed (M-2); with `17e1e0e` it shows `current` |

## Findings

### Must-fix

**M-1 (fixed, `537ecf6`, decision 238) - A template warmed from a Windows checkout never matches a task.** With
`core.autocrlf=true` the checkout's `Cargo.lock` has CRLF, and `Locks::Dir` hashed those bytes; a task reads the
commit's LF blob through `Locks::Commit`. So the README's own recipe (`--warm <this repo>`) made a strict profile refuse
every task (`the lock files of commit e7e1b14 ... differ`). The lock hash now reads CRLF as LF; test
`a_crlf_checkout_of_a_lock_file_hashes_like_its_lf_commit` (`tests/task_template.rs`), red first.

**M-2 (fixed, `17e1e0e`) - `sbxm list` shows every template sandbox as "config changed".** `config_status` compared
the stored hash with `config_hash` only, while a template sandbox records `template_config_hash` (decision 230), which
`open` checks. `list` told the user to `--rebuild` a current sandbox. `GlobalConfig::current_hash` now takes the
variant; test `list_checks_a_template_sandbox_against_the_template_variant` (`tests/new_template.rs`), red first.

### Should-fix (for the end-of-milestone review)

**S-1 - The warm-up prints nothing for minutes.** Between `CREATE SANDBOX` and the stop, `template build --warm` runs
`cargo fetch`, clippy and `cargo test --no-run` with no output (about 4 minutes here, longer on a bigger repo). A line
per step would show it is working.

**S-2 - `config show` prints the install variant's hash for a project whose sandbox came from a template**
(`src/commands/config_show.rs:42`), so it doesn't match `state.json`. Display only; `open` and now `list` check the
right variant.

### Notes

- Order changed: step 7's codex template was built before step 5, because a strict profile refuses the task's Codex
  reviewer without one. Worth a line in the docs: a strict profile needs a template for every harness a task uses.
- `build-known.ps1` also repoints the user's normal wrapper `H:\sbxm-e2e\sbxm.ps1`; harmless here.
- Cleanup: the scratch sandboxes, the task and both templates are removed; `sbx ls` shows no sbxm sandbox.
