206. **Template commands and users (2026-10-15, the user agreed with the recommendation):** new commands `sbxm
template build <profile> [--harness h] [--warm <repo>] [--ignore-disk]` (`--harness` defaults to `claude`;
`--ignore-disk` as on every command that creates a sandbox, [194], added after the PR 253 review round 2, S-1), `sbxm template ls` and
`sbxm template rm <profile> [--harness h]`. Every command that creates a sandbox follows the profile's opt-in [202]:
`new`, `open --rebuild`, every `sbxm task` sandbox (worker, reviewer, fix rounds) and `sbxm run`'s contestants and
judge; no `--no-template` flag until a need shows. sbxm's template records live in
`<base>/.sbxm/templates/<profile>-<harness>.json` (tag, template hash, lock file hash, `sbx` version, size, date),
beside the other machine-local state, not in the config dir. The template a sandbox was created from is recorded in
`state.json` and `task.json` (what [205](b) checks). For a task, the lock file check of [203](b) reads `Cargo.lock`
from the task's base commit in its `repo.git`, so no host git runs in the agent's clone [159]. Defining sandboxes
as today stays fully supported (the user asked, 2026-10-15): a profile without `[template]` is unchanged [202], and
a non-strict profile falls back to today's installs.
