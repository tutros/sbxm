# Milestone 2a code review

Scope: `origin/main..HEAD` (`8aec45b1ecdede447a7e594c447d7d6530e40ec8..82659a26614c58da671641ffa6e62264cb9038ce`, 22 commits).

Review type: end-of-milestone full checklist plus the cross-cutting sweep from `sdlc-code-review`. Read first: `AGENTS.md`, `milestone-2.md`, `decisions.md` (especially 124–133), and `batches/2026-09-30-m2a-unattended.md`. The changed production functions were then read as whole functions, with focused sweeps over harness matches, validation-before-write, persistence, cleanup, paths, config hashing, evaluator siblings, docs, and tests.

Issues: M2A-1 #41, M2A-2 #42, M2A-3 pending, M2A-Q1 pending

## Summary

- Must fix: 2
- Should fix: 1
- Questions: 1

This branch is not yet a complete M2a delivery: the batch record says slice 12 (cosine), slice 13 (the manual end-to-end check), and slice 14 (Jev) were not attempted. The findings below cover the committed range; the end-of-milestone review cannot be the final release gate until those planned slices are either completed or explicitly removed/deferred by a user-confirmed decision, and the manual check is recorded.

## Must fix

### M2A-1 — Inherited Git config variables can execute a host command while diffing a contestant workspace

**Where:** `src/git.rs:50-72`, used by `src/git.rs:99-100` and `src/run/diff.rs:23-34`.

**What happens:** `git::output` says that nothing inherited may make Git run code, but removes only eight named variables. Git's environment-based config variables (`GIT_CONFIG_COUNT`, `GIT_CONFIG_KEY_*`, and `GIT_CONFIG_VALUE_*`) remain inherited. A reproduction using the production `git add -A` arguments, a workspace `.gitattributes` containing `* filter=reviewprobe`, and inherited config defining `filter.reviewprobe.clean` created a marker file on the host; the command reported `marker_test_exit=0`. Thus the supposedly hardened host-side Git invocation can execute an inherited filter program while reading contestant-controlled attributes.

**Why it matters:** decision 127 explicitly requires “no inherited `GIT_*` variables” because host Git processes an adversarial contestant workspace. This is a security boundary and therefore meets must-fix criterion 1 (security risk and contradiction of a confirmed decision). Existing hostile-repository tests cover `.git/config`, but not environment config.

**Smallest fix:** construct the Git child with a cleared environment and restore only the non-Git variables Git actually needs, or remove every inherited variable whose name starts with `GIT_` before setting the fixed safe values. Add a test that sets `GIT_CONFIG_COUNT`/key/value in-process, plants the filter attribute, runs the public seeded-diff behavior, and proves no host command runs while the diff still succeeds.

**Acceptance criteria:**
- [ ] With `GIT_CONFIG_COUNT=1`, `GIT_CONFIG_KEY_0=filter.probe.clean` and `GIT_CONFIG_VALUE_0` set to a command that creates a marker file, plus a workspace `.gitattributes` containing `* filter=probe`, the seeded diff (`run::diff::seeded`) succeeds with the expected patch and the marker file does not exist.
- [ ] The same holds for the baseline commit (`git::baseline_commit` runs the same `git add -A`, `src/git.rs:99-100`), so the seeding path is covered too.
- [ ] The same holds for the unseeded diff (`git diff --no-index`), or the issue records why it cannot run a config-defined command.
- [ ] A unit test on the built command shows that no inherited `GIT_*` variable survives except the fixed values sbxm sets; it sets `GIT_CONFIG_PARAMETERS` as well as `GIT_CONFIG_COUNT`, so the whole family is covered, not only the variables the review names.
- [ ] `git` still runs on the Windows host after the change (it needs `SystemRoot` and `PATH`); the real `cargo test` run on the host is the evidence.
- [ ] A test covering it fails before the fix and passes after (name it)
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Docs updated where behavior users see changed, or "no user-visible change"
### M2A-2 — `[eval.cosine]` is accepted but silently does nothing

**Where:** `src/run/config.rs:53-59`, `src/run/config.rs:133-142`, and `src/run/config.rs:366-371`; the acceptance is asserted at `tests/run_config.rs:65-90`.

**What happens:** a run-config with `[eval.cosine]` deserializes successfully and stores `EvalConfig.cosine`, but no runtime module reads that field (`rg -n cosine src tests` finds only schema/test/comments). `sbxm run` therefore completes without computing or saving similarity and without warning that the configured evaluator was ignored. The batch record confirms slice 12 is “not started.”

**Why it matters:** this is the exact silent-drop case prohibited by decision 11 and by checklist section 3. It produces wrong behavior now and nothing in the accepted config tells the user that the requested evaluation was skipped, so it meets must-fix criteria 1 and 3.

**Smallest fix:** until slice 12 is implemented, reject `[eval.cosine]` with the same explicit “not supported yet” pattern used for unimplemented schema such as Jev. Slice 12 can then make the config valid together with the evaluator and tests proving `evals.json` receives its output.

**Acceptance criteria:**
- [ ] `RunConfig::load` on a config with `[eval.cosine]` fails with one line naming the file, in the `<problem>; <fix>` form, for example `run.toml: [eval.cosine] isn't implemented yet; remove it`; the exact text is fixed in the test.
- [ ] The test at `tests/run_config.rs:65-90`, which asserts acceptance today, is changed first to assert that rejection, and the test at `tests/run_config.rs:153` (an unknown key inside `[eval.cosine]`) is updated or removed.
- [ ] `sbxm run` with such a config exits non-zero before any write: no `.sbxm/runs/` folder, no kits and no backend call (asserted with `FakeBackend`).
- [ ] `README.md` does not describe `[eval.cosine]` as working (it does not mention it today, so this is a check).
- [ ] A test covering it fails before the fix and passes after (name it)
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Docs updated where behavior users see changed, or "no user-visible change"
## Should fix

### M2A-3 — One implementation commit is not a green step

**Where:** `batches/2026-09-30-m2a-unattended.md:127`; commit `8db96f4` (`Add hardened seeding and diff capture`) precedes its wiring commit `d99f53c`.

**What happens:** the batch report explicitly records “the first commit of slice 7 not compiling on its own.” That conflicts with the implementation and review rules requiring every behavior commit to be a green step after formatting, clippy, and tests.

**Why it matters:** a bisect or review at that commit cannot build, and the milestone history does not provide the promised independently verifiable steps. It does not change current tip behavior, so it is should-fix rather than must-fix.

**Smallest fix:** before merge, combine the two slice-7 commits into one green commit, or otherwise reorder/split them so every resulting commit builds and passes the required checks. Because the branch is already pushed, coordinate any history rewrite rather than force-pushing unexpectedly.

## Question

### M2A-Q1 — Should comparison configs continue accepting Antigravity while fresh run sandboxes cannot authenticate?

**Where:** `decisions.md:199-200` (decisions 124f and 125), `src/run/config.rs:395-410`, and `src/run/preflight.rs:61-78`.

**What happens:** decision 124f records that a fresh Antigravity sandbox fails authentication, and that the provisional required `google` secret is ignored by `agy`. Nevertheless Antigravity is an allowed comparison harness; preflight requires that ineffective secret and the run then predictably records an authentication failure. The batch leaves the authentication route explicitly open.

**Why it matters:** the current behavior is documented as provisional rather than an accidental departure, so this needs the user's decision rather than being relabeled as a defect. It blocks a successful three-provider M2a end-to-end check.

**Recommendation:** reject Antigravity contestants at preflight with an actionable “authentication for throwaway run sandboxes is unresolved” error until an authentication route is confirmed; keep the adapter and fake/fixture coverage in place. If deliberate failed contestants are useful, add an explicit opt-in rather than treating the harness as normally runnable.

## Verification and checklist notes

- The focused Git reproduction passed in the sense that it reproduced M2A-1: the inherited config caused the filter command to create its host marker.
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test --no-fail-fast` could not be run in this review environment because `cargo` is not installed/on `PATH` (`/bin/bash: cargo: command not found` for all three). The batch log reports green checks at each slice tip and real-`sbx` checks for the relevant slices, but this review could not independently confirm them.
- No real `sbx` command was run. The manual slice-13 end-to-end check remains outstanding by the batch's own record.
- Cross-cutting harness sweep: lifecycle matches include Antigravity; comparison configs deliberately restrict contestants/judges to Claude, Codex, and Antigravity; provider, kit, mandatory-instruction, unsupported-feature, parsing, hash, and display branches were checked across those siblings. No additional evidence-backed omission was found.
- Validation/write ordering, pair and judge cleanup, per-pair persistence, run-ID validation, profile-specific kits/hashes, timeout recognition, result/report paths, and README/AGENTS updates were checked against decisions 124–133. No additional must-fix was established.
- Nit: `AGENTS.md` still describes `SandboxBackend` without its new `exec` and `skills` methods; fold that small documentation correction into the next docs update rather than opening a separate finding.
