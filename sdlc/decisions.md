# Decisions

Running log of design decisions for model_compare. See `sdlc/idea.md` for the original brief.

## Round 1 (2026-09-24)

1. **Sandbox contents:** Each sandbox runs a full agent harness (as Docker Sandboxes does today), with no loss of functionality. Comparisons can vary both harness and model, e.g. two models on the same harness plus a third model on a different harness.
2. **Docker mechanism:** Use Docker Sandboxes (`sbx`) for now. Investigate the `bollard` crate later.
3. **Captured output:** Capture all three: final answer, workspace diff, and full transcript. Not every eval applies to every comparison (e.g. no cosine similarity on code diffs).
4. **Eval criteria:** Both a free-text rubric and executable checks. JEV support will likely be added later.
5. **Starting state:** Each contestant gets either an empty directory or a fresh copy of an optional seed directory, under `<base>/<project>/runs/<run-id>/<contestant>/`.
6. **Execution:** Comparisons run headless and in parallel, with a timeout and a cost/token cap per contestant. Standalone sandboxes are interactive.
7. **Providers/auth:** Anthropic, OpenAI and Google at first; OpenRouter and Pi later. Prefer OIDC auth and fall back to API keys when OIDC won't work. Keys are defined with `sbx secrets`.
8. **v1 scope:** Milestone 1 is sandbox lifecycle (create/reuse a project sandbox from the common config). Model comparison is milestone 2.

## Round 2 (2026-09-24)

9. **JEV** = Jev, TypeSafe's System One model (docs: https://docs.typesafe.ai/llms.txt). **Pi** = the Pi AI coding harness (not a provider).
10. **Contestant** = `(harness, model, optional config variant)`.
11. **Config across harnesses:** Option (c) for now: shared instructions file, shared egress/secrets, hooks/guardrails per harness. Long-term goal is (a): one canonical config with per-harness adapters. Warn loudly when a harness can't support a configured feature.
12. **Caps/auth:** Caps are best effort (harness flags where available); the timeout is the only guaranteed limit. Standalone sandboxes can use existing subscriptions. Comparisons may later require API keys plus budgets (undecided).
13. **Headless `sbx`:** Unknown. Prompts can be passed to a sandbox, but headless runs are untested. Run a spike at the start of milestone 2, and check the `sbx` CLI surface during milestone 1.
14. **Diff capture:** Always snapshot fresh: copy files, new git repo, one baseline commit. No seed history or remotes. Security and isolation are core goals.
15. **Repetitions:** `--repeat N` is in the data model from day one; default 1.
16. **Timeout/crash:** Keep partial output and mark it `timed_out`. Checks and judges still evaluate it.
17. **Backend:** Shell out to `sbx` behind a `SandboxBackend` trait; `bollard` can become a second backend later.

## Round 3 (2026-09-24)

18. **Rubric format:** Structured. Each criterion has `id`, `kind` (`pass_fail` → Jev Noul, `scale` with described levels → Jev Score), `weight` and optional free-text `notes` for the LLM judge and humans. All evaluators score against the same criteria.
19. **Scoring mode:** Score each contestant separately by default and rank in code. Direct comparison is optional (the LLM judge's winner call), with randomized order and blind labels.
20. **Evaluator inputs:** Jev sees the final answer plus the diff (split per file if over the 32k budget), never the transcript. The LLM judge sees the answer plus the diff, and optionally a transcript summary. Humans see everything. **Also allow Jev-specific criteria** (raw Noul/Choice/Score questions defined alongside the shared rubric). Expect to revisit this as eval-writing practice matures.
21. **Judge fairness:** Judge model is set per comparison, with a warning when it shares a provider with a contestant. Contestant labels are anonymized (A/B/C) for the judge and humans; the mapping is revealed only in the final report.
22. **Human review:** CLI first (or just readable reports); a local web page later.
23. **Embeddings:** Local (`fastembed`): no secrets, no cost, deterministic. Text answers only.
24. **Results storage:** Files are the source of truth (`runs/<run-id>/<contestant>/{answer.md, diff.patch, transcript.jsonl, evals.json}`). SQLite can be added later as a rebuildable index.
25. **Harness adapters:** We define an adapter per harness (headless command, transcript parsing, native config file names), whether or not `sbx` supports the harness natively. Pi runs through the official Pi kit for `sbx`.

## Round 4 (2026-09-24)

26. **sbx kits:** Unknown whether a kit can carry instructions, hooks, egress rules and secret references. Part of the milestone 1 spike. If it can, compile the common config into a kit instead of applying it at launch.
27. **Config format:** TOML with three layers: global (`~/.config/sbxm/config.toml`) → named profile (`default`, plus others such as `strict`) → project (`<base>/<project>/sandbox.toml`). Comparison config variants reference profiles.
28. **Config versioning:** The common config lives in its own directory, ideally a git repo. Every sandbox and run records a hash of the fully merged config.
29. **Egress:** Deny by default. The harness adapter automatically adds its provider's domains. Check `sbx` network policy support in the spike.
30. **Secrets:** Config names secrets and never contains values. Verify each named secret exists before launch. Secret values are never written to results. Scan transcripts and diffs for secret values if `sbx` exposes them; otherwise rely on `sbx` redaction.
31. **Existing project:** Attach if the sandbox is running, recreate it if gone. If the merged config hash changed, warn and require `--rebuild`.
32. **M1 commands:** `new`/`open <project>`, `list`, `stop`, `rm` (deletes the directory only with `--purge` plus confirmation), `config show`, `doctor`.
33. **Project names:** `[a-z0-9-]` only. Reject `..`, path separators and reserved Windows names.
34. **Name:** CLI, binary and crate are `sbxm`.
35. **Testing:** Tests run against a fake `SandboxBackend` by default; real-`sbx` tests sit behind a feature flag or `--ignored`.

## Round 5: after spikes S1–S4 (2026-09-24)

36. **sbxm's role:** `sbx` enforces egress, secrets and isolation. sbxm compiles its TOML config into v2 kits, calls `sbx create --kit …`, and keeps host-side state (projects, runs, config hashes) plus evals. sbxm does no enforcement of its own.
37. **Mandatory vs. reference instructions:** Each per-harness mixin (`requires.agent`) writes mandatory rules to that harness's always-loaded user-level instructions file (e.g. `~/.claude/CLAUDE.md` via `files/home/`). The kit `agentInstructions` field is only for optional reference material, because `sbx` surfaces it on demand. Paths for Codex, Gemini CLI and Pi are unverified.
38. **`sbx env`:** Not used for now; sbxm calls `sbx create` directly. Generating `sbxenv.yaml` may come later.
39. **Kit format risk:** Accepted. Generate v2 kits from a single module, and `doctor` enforces a minimum `sbx` version (currently 0.43.0).

## Round 6: milestone 1 plan (2026-09-24)

40. **Metadata location (P1, changes paths in 5, 24, 27):** The workspace is `<base>/<project>`. All sbxm metadata lives in `<base>/.sbxm/<project>/` (`sandbox.toml`, `state.json`, generated kits, and in M2 `runs/`), which is never mounted into a sandbox, so agents can't read or edit their own config.
41. **Sandbox names (P2):** `sbxm-<project>-<harness>`. One project can have one sandbox per harness.
42. **Repo directory (P3):** Rename `model_compare/` to `sbxm/` (done by the user outside the session, since VS Code holds the folder open). Git repo initialized on `main` (2026-09-24); no commits yet. The scaffold in milestone 1 step 1 will be the first commit, together with the planning docs.

## Implementation (2026-09-24)

43. **Starter config (`config init`, slice 1):** Writes `<config_dir>/config.toml` and `<config_dir>/profiles/default/profile.toml`, where `<config_dir>` is `SBXM_CONFIG_DIR` or `~/.config/sbxm`. `profiles_dir` defaults to `<config_dir>/profiles` (so the config dir is the directory to version), and `base_dir` defaults to `~/sbxm-projects` (under home, not AppData). The starter profile references no files. If either file exists, nothing is written.
44. **Project name length (slice 2):** At most 40 characters, so `sbxm-<project>-<harness>` stays well under the 63-character hostname limit (longest harness today: `gemini`, 52 characters total).
45. **Testability and state shape (slice 3):** The crate is lib + bin. Command functions take the config dir and a `&dyn SandboxBackend`, so tests run them in-process with `FakeBackend` (no env vars, no test switches in the binary). `state.json` is `{"sandboxes": {"<harness>": {sandbox, workspace, created_at}}}` (unix seconds), keyed by harness per decision 41; profile, hash, `sbx` version and kit paths are added by the slices that produce them. Real-`sbx` tests are `#[ignore]` in `tests/real_sbx.rs` and read `SBXM_REAL_BASE_DIR`.
46. **Skills (2026-09-24):** `sbx` keeps its own skills store (filled explicitly with `sbx skills import`/`add`), mounted at each agent's skills dir with `sbx create --skills readonly|readwrite|off` (default `readonly`). It does not expose the host's `~/.claude/skills` directly. A profile setting `skills.store = "readonly" | "off"` (default `readonly`) maps to `--skills`. `readwrite` is rejected, because an agent could plant skills that every other sandbox loads. The store serves Claude, Codex, Copilot, Cursor, Droid and OpenCode, not Gemini CLI or Pi. **Later:** profile-owned skills (`skills.dir`) copied into each harness mixin's `files/home/<skills dir>`, versioned and included in the config hash, with `--skills off` for that sandbox; pending spike S6. **M2:** record `sbx skills ls --json` with each run so store drift is visible.
47. **Seeding `new` (slice 4):** `--seed` only creates a new project (an existing workspace is an error). The seed is checked before anything is written: it must be a directory, must not contain the base dir (the copy would recurse), and must not contain symlinks or junctions (following one could copy files from outside the seed into the sandbox). Contents are copied as-is, including any `.git`; the fresh-repo-with-baseline-commit rule applies to M2 comparison seeds.
48. **Pi and `kit.allowedSources` (after S6; superseded by 73 for Pi):** `sbx` only installs kits from `kit.allowedSources` (default `["docker.io/"]`); the Pi kit is on `github.com/docker/`. sbxm never changes `sbx` settings. `doctor` reports it when a profile or command needs Pi, and `new`/`open --harness pi` fail before creating anything. Like other sbxm errors, the message says what's wrong and how to fix it, built from the current setting, e.g.: `the Pi kit's source github.com/docker/ is not in sbx's kit.allowedSources; allow it with sbx settings set kit.allowedSources '["docker.io/","github.com/docker/"]' (this trusts every kit from github.com/docker/)`.
49. **Harness home files (after S6):** Verified: Claude loads `~/.claude/CLAUDE.md` (canary answered by `claude -p`, also after restart); Codex puts `~/.codex/AGENTS.md` in its prompt (`codex debug prompt-input`, no model call); Gemini CLI uses `~/.gemini/GEMINI.md` (shipped docs and code; loading unverified, no credentials). Skills dirs: Claude `~/.claude/skills/`; Codex `~/.codex/skills/` and `~/.agents/skills/`; Gemini `~/.gemini/skills/` and `~/.agents/skills/` (`.agents` wins on a name clash). A mixin's `files/home/.claude/CLAUDE.md` and skills survive create and restart, but the Claude kit **replaces** a mixin's `~/.claude/settings.json` (no merge), so hooks can't ship as that file; the route is open (S6b). With an empty skills store, `--skills readonly` mounts nothing. Pi paths are unverified (blocked by [48]).
50. **`open` on a sandbox without sbxm state (slice 9):** Refuse, with a message naming the sandbox and how to remove it (`sbx rm <sandbox>`, which deletes its session history) before running `sbxm open` again. sbxm doesn't attach to a sandbox it has no record of creating, since it may lack sbxm's config. Confirmed by the user 2026-09-24.
51. **Profile loading (slice 10a):** Unknown keys in a profile are errors (serde `deny_unknown_fields`), so a typo such as `[netwrk]` or a section a later slice will support is never silently ignored [11]. Profile names use the project-name characters (`default` allowed). In the global config, `profiles_dir` defaults to `<config_dir>/profiles` and `default_profile` to `default`, so configs without them still load.
52. **Kit generation (slice 10b):** `spec.yaml` is written with `serde_norway` (maintained fork of `serde_yaml`; user's choice over emitting JSON). The `common` mixin is named `sbxm-common` and lives in `.sbxm/<project>/kits/common/` until slice 11 adds the hash prefix. It's validated with `sbx kit validate --json` before anything else is created; an invalid kit aborts with sbx's reason and the profile to fix. `rm` deletes the generated `kits/` with the project's last sandbox. Verified live: kit-allowed `example.org` 200, `example.com` 403.
53. **Profile env (slice 10c):** `[env]` becomes the `common` mixin's `environment.variables` (sorted; omitted when empty). sbxm checks it when loading the profile: names must be shell identifiers, and values can't contain `${{` (sbx reads it as a kit expression). sbx rejects both too, but its messages blame "the kit's author"; sbxm's name the profile. Verified live with `printenv`.
54. **Skills store flag (slice 10d):** implements [46]. sbxm always passes `--skills readonly|off` explicitly, because without it `sbx` falls back to its own `skills.defaultMode` setting, which could be `readwrite`. `readwrite` and unknown values are rejected when the profile loads. Live `sbx create` output shows the store mapped as `…agent-skills → /home/agent/.claude/skills · 0 folders · (ro)`: input for S6b's non-empty-store question.
55. **Config hash (slice 11, confirmed by the user 2026-09-25):** implements [28] and [31]. The hash is SHA-256 over canonical JSON of the profile name, the parsed profile *without* `description` (it never reaches the sandbox, so editing it doesn't force a rebuild), the global `resources`, and sbxm's version; referenced files join it in slices 14–15. State stores the full hex hash; kit dirs use a 12-character prefix (`kits/<prefix>/common/`), so a new config never overwrites the kit an existing sandbox was built from. State written before slice 11 has no hash and counts as changed, so `open` needs `--rebuild`. Profile `env` names starting with `SBXM_` are rejected at load: the prefix is reserved for sbxm's own variables (`SBXM_CONFIG_HASH`, `SBXM_PROFILE`), and letting either side win silently would break [11]. After a successful `open --rebuild`, the previous kit dir is deleted if no other state entry of the project uses it.
56. **Workspaces on `C:` (correction to S1, 2026-09-25):** On this machine (sbx v0.43.0, Docker Desktop, Windows 10), `sbx create` fails with `ERROR: failed to run sandbox container` for *any* workspace on `C:`, not only under `%TEMP%`/AppData: `C:\Users\james\sbxm-projects\demo`, `C:\sbxm-diag`, and plain `sbx create shell C:\sbxm-diag` (no sbxm, no kit) all failed; the same sbxm arguments with the workspace on `E:\` worked, and memory (2g vs 8g) made no difference. The cause is unverified (Docker Desktop file sharing is a guess). Consequences: the starter `base_dir` default `~/sbxm-projects` (decision 43) is on `C:` on Windows and needs changing; `doctor` should flag a base dir on a drive `sbx` can't mount, not just AppData. Both are carry-over items in `sdlc/milestone-1.md`. **Update 2026-09-26:** the user reproduced `sbx create --name sbxm-diag-c shell C:\sbxm-diag` failing the same way in their own terminal, although they have created sandboxes on `C:` before. They suspect low free space on `C:` (unverified) and see it as a problem with their machine, not sbxm: the starter `base_dir` and a `doctor` check for `C:` are **deferred**, not planned. `doctor`'s free-disk-space check (Commands table) stays.
57. **Project overrides (slice 12, confirmed by the user 2026-09-25):** implements [27] with the path from [40]: `<base>/.sbxm/<project>/sandbox.toml` (optional) has the same keys and load-time checks as a profile, and errors name that file. It's merged on top of the profile: lists append (`network.allow`, `network.deny`, `secrets.services`); maps merge per key, project winning (`env`); scalars override when set (`skills.store`). A project can't remove anything its profile allows. The merged result is what's hashed [55], so editing `sandbox.toml` shows as drift.
58. **Secrets check (slice 13, confirmed by the user 2026-09-25):** implements [30]. Before writing anything, `new` (and `open` when it creates or rebuilds) checks each merged `secrets.services` name against `sbx secret ls --json`, counting only entries with `scope: global` and `type: service`. Sandbox-scoped secrets can't exist before sbxm creates the sandbox, and host env vars `sbx` has found but not imported (`env_only_count`, `sbx setup`) don't reach the sandbox, so neither counts. A missing secret fails with the fix (`sbx secret set <service>` or `sbx setup`); sbxm never sets secrets [48]. The output's shape (`secrets`, `custom_secrets`, `shadowed_services`, `env_only_count`; each secret has `scope`, `type`, `name`, a masked `secret`) was read with values redacted; the fixture is reconstructed from it, because printing the raw output was blocked by a permission check.
59. **Claude hooks route (after S6b, confirmed by the user 2026-09-25):** Hooks from a profile reach Claude through **managed settings**: the `harness-claude` mixin gets a `setup.install` step (`user: "0"`) that writes `/etc/claude-code/managed-settings.json` (`files/` can't target `/etc`, SPEC-v2). Verified in S6b: the hook fires in `claude -p`, and the file survives stop/start. Chosen over merging into `~/.claude/settings.json` with `jq` (also verified working) because managed settings take precedence over the user and workspace settings the agent can edit without `sudo`. **Not tamper-proof:** the agent has passwordless `sudo` in the Claude image, so this is a strong default, not a lock. `settings.local.json` doesn't work (Claude Code 2.1.280 ignores its hooks).
60. **Non-empty skills store (after S6b, confirmed by the user 2026-09-25):** left open. S6b Q1 was blocked: `sbx skills add` only takes remote Git URLs, and importing a canary from a fake home was refused by the permission check. Only profile-owned skills (decision 46, not in M1) depend on it, and they already plan `--skills off`, which avoids the question. Revisit if profile-owned skills are built.
61. **Harness config files in `home_files` (after S6b, confirmed by the user 2026-09-25):** a profile's `harness.<h>.home_files` may not contain the harness kit's own config file: `.claude/settings.json` and `.codex/config.toml` are replaced by the kit (the mixin's version is lost), and a mixin's `.gemini/settings.json` wins but silently drops the kit's defaults (`tools.sandbox=false`, `folderTrust.enabled=false`). Either way something is silently dropped [11], so those paths are an error when the profile loads, naming the file and, for Claude, pointing to hooks via managed settings [59]. Instruction files are fine: a mixin's `~/.claude/CLAUDE.md`, `~/.codex/AGENTS.md` (in Codex's rendered prompt) and `~/.gemini/GEMINI.md` survive create and restart (S6, S6b).
62. **Instruction files (slice 14, confirmed by the user 2026-09-25):** implements [37]. `instructions.mandatory` and `instructions.reference` are paths relative to the file that names them (the profile's folder, or `.sbxm/<project>/` for `sandbox.toml`), and must stay inside it: absolute paths and `..` are errors naming the file, and nothing is created. This keeps a profile self-contained in the versioned config [28]. The files' *contents* (not their paths) join the config hash [55]; a project's value overrides the profile's [57]. `mandatory` goes to `files/home/.claude/CLAUDE.md` in a `harness-claude` mixin (`name: sbxm-harness-claude`, `requires.agent: claude`), generated for every sandbox and passed as the second `--kit` after `common`; `reference` goes to the common mixin's `agentInstructions.content`. Verified with `sbx kit validate` (v0.43.0): `agentInstructions: {content: …}` is valid (a plain string is not), and a mixin with only `requires.agent` is valid.
63. **Claude home files (slice 15b, confirmed by the user 2026-09-26):** `harness.claude.home_files` names a folder whose contents are copied into the `harness-claude` mixin's `files/home/`. Like instruction files [62], the path is relative to the folder of the file that names it and must stay inside it, and a project's value replaces the profile's (no merge of folders). The folder may not contain symlinks or junctions (as for seeds [47]). Every file's relative path and contents join the config hash. `.claude/settings.json` is an error [61], and so is `.claude/CLAUDE.md` while `instructions.mandatory` is set, since one would silently replace the other [11]. All checks run when the config loads, before anything is written.
64. **Claude managed settings (slice 15c, confirmed by the user 2026-09-26):** implements [59]. `harness.claude.managed_settings` names a JSON file (path rules as in [62]; a project's value replaces the profile's). It must hold a JSON object, and is written to `/etc/claude-code/managed-settings.json` by a `setup.install` step (`user: "0"`) in the `harness-claude` mixin. It's Claude's own settings format, so it covers hooks and anything else managed settings support (e.g. permission rules), and sbxm keeps no hooks schema of its own. sbxm re-serializes it as one compact JSON line with `$` escaped as `\u0024`, written through a quoted heredoc: nothing can end the heredoc early or form a `${{` kit expression. The compact form is what's hashed, so reformatting the file doesn't cause drift.
65. **`config show` (slice 16, confirmed by the user 2026-09-26):** prints exactly the hash input [55] as TOML (resources, network, env, secrets, setup steps, skills store, full instruction texts, the compact managed settings, and each home file as `path = sha256`), under comment lines naming the profile, the project (if given) and the config hash. That way the output explains any drift, and sbxm keeps no second format. With a project, `sandbox.toml` is merged in, and the profile is `--profile`, else the one recorded in the project's state (as `open` and `list` use), else `default_profile`, so the printed hash is the one `open` compares. `--kits` also prints each generated `spec.yaml` and its home file paths. Nothing is written and `sbx` isn't called. `--harness` waits for slice 18.
66. **What `doctor` checks (slice 17, confirmed by the user 2026-09-26):** every profile in `profiles_dir`, plus every existing project with its `sandbox.toml` merged over the profile recorded in its state (the pairing `open` and `list` use). For each, the config must load (which covers referenced files), its secrets must be stored, and its generated kits must pass `sbx kit validate`. The Pi `kit.allowedSources` check [48] waits for slice 20.
67. **`doctor`'s base dir checks (slice 17b, confirmed by the user 2026-09-26):** the base dir must exist, be writable (a probe file is created and removed), not be under `%TEMP%`/`%TMP%`/`%APPDATA%`/`%LOCALAPPDATA%` (S1), and have at least **10 GiB** free on its drive. Free space comes from the `fs4` crate (`available_space`), since `std` can't report it. The host probes (free space, writability, temp dirs) are passed in, so tests don't depend on the machine.
68. **`--harness` (slice 18, confirmed by the user 2026-09-27):** accepts `claude` and `codex` for now (default `claude`); `gemini` and `pi` are rejected until slices 19–20 (`gemini` added in slice 19). There is no `default_harness` global setting until something needs it. Slice 18 is cut into 18a (`new --harness codex` with a `harness-codex` mixin writing mandatory instructions to `~/.codex/AGENTS.md` [49], and `state.json` merged per harness), 18b (loud warning when Claude-only settings are configured for a Codex sandbox [11]) and 18c (`--harness` on `open`, `stop`, `rm` and `config show`).
69. **Config hash per harness (slice 18c, confirmed by the user 2026-09-27):** refines [55]. The hash input includes the harness name, and only settings that reach that harness: a Codex sandbox's hash leaves out `harness.claude.*`. A Claude-only edit then shows drift on the Claude sandbox alone, instead of forcing a needless `--rebuild` (which loses session history) of the Codex one. Claude and Codex kits of one project land in separate hash folders; `common` is written twice. Adding the harness name changes existing Claude hashes once, so each existing sandbox needs one `--rebuild`.
70. **`rm` with several harnesses (slice 18c, confirmed by the user 2026-09-27):** plain `rm --harness <h>` removes that sandbox and its state entry, and deletes its `kits/<hash-prefix>/` unless another of the project's sandboxes uses the same hash (the rule `open --rebuild` uses [55]); the last sandbox still takes all of `kits/` with it [52]. `rm --purge` is project-wide, because the workspace is shared: it removes **every** sandbox of the project, and the confirmation lists them along with the folders. `--purge` with `--harness` is an error.
71. **`doctor` and harnesses (slice 18, confirmed by the user 2026-09-27):** refines [66]. For each project, `doctor` checks the config, secrets and kits of every harness recorded in its state, with that harness's hash and kits [69], and each line names the harness (`project demo (codex, profile 'default')`). Bare profiles are still checked with Claude only, until a profile can say which harnesses it's for.
72. **Skills store and Gemini (slice 19, requested by the user 2026-09-27):** the `sbx` skills store doesn't serve Gemini CLI [46], so `new`/`open` warn for a Gemini sandbox whenever `skills.store` isn't `off` (including the default `readonly`), saying the store's skills don't reach it and that `skills.store = "off"` silences the warning. `--skills` is still passed, and `skills.store` stays in Gemini's hash.
73. **Pi kit source (after S7, confirmed by the user 2026-09-27):** supersedes [48] for Pi. sbxm uses the Pi kit from Docker Hub, pinned to an immutable tag: `docker.io/sbx/pi-kit:20260924-d058fedc156325f87612d9bcd9bd313ab74ba100` (tags are `<YYYYMMDD>-<full sbx-kits-contrib SHA>`; `latest` had the same digest). `docker.io/` is in `sbx`'s default `kit.allowedSources`, so no allowlist change is needed, and sbxm has no allowlist check or error for Pi (`new` and `doctor`). Verified with the allowlist overridden to `["docker.io/"]`: the tag creates with an sbxm mixin (`requires.agent: pi`), while the GitHub ref is refused (`sdlc/spikes/S7-results.md`, follow-up check). Changing the tag is a code change. The kit's image (`docker.io/sbx/pi-image:latest`) is rebuilt nightly, so Pi's own version isn't pinned.
74. **Pi credential binding (after S7, confirmed by the user 2026-09-27):** the Pi kit declares its own `credentials` block, and `sbx` only injects a stored secret into such a kit once the user has a binding (`~/.config/sbx/credentials.yaml`, SPEC-v2 §5.4); otherwise it prints `no binding authorizes anthropic — the credential was not injected` and model calls fail with 401. The file's format is undocumented and has no CLI, so sbxm doesn't check bindings in `new` or `doctor`. `SbxBackend` runs `sbx create` attached to the terminal, so `sbx` should ask the user itself (unverified). At the end of slice 20 the user runs `sbxm new <test project> --harness pi` in their own terminal and answers once; the main session then records which keys `credentials.yaml` gained (no values) and checks the credential reaches the sandbox. **Verified 2026-09-27:** run from the user's terminal, `sbxm new pi-test-project --harness pi` showed `sbx`'s prompt (`This kit wants to use these credentials: … [A]pprove all · [R]eview each · [N]o`). After approving, `sbx` printed `Binding saved to C:\Users\james\AppData\Roaming\sbx\credentials.yaml` (on Windows it's `%APPDATA%\sbx\`, not `~/.config/sbx/`). The file holds no secrets, only approved domains per service: `bindings.anthropic.apiKey.domains: [api.anthropic.com]` and `bindings.anthropic.oauth.domains: [platform.claude.com, api.anthropic.com]`. The create printed no binding note, and `SBX_CRED_ANTHROPIC_MODE` in the sandbox is `apikey` (it was `none` without the binding). The user then opened the sandbox (`sbxm open pi-test-project --harness pi`) and got an answer from an authenticated Claude model in Pi.
75. **Reference instructions in Pi (after S7, confirmed by the user 2026-09-27):** refines [37] for Pi. `sbx` writes a kit's `agentInstructions` as `AGENTS.md` in the workspace's parent folder inside the container, and Pi loads context files from every parent folder, so for Pi they're always loaded, not on demand. sbxm still sends `instructions.reference` to Pi sandboxes, and `new`/`open` warn when it's set for one, saying it's always loaded there [11]. Whether other harnesses read that file is unverified. A future `harness.pi.home_files` would refuse `.pi/agent/auth.json`: it outranks the environment in Pi's credential order and `sbx` rewrites it for OAuth hosts (S7 Q5; as [61]).
76. **No `default_harness` setting (after slice 21, confirmed by the user 2026-09-27):** follows [68]. `config init` wrote `default_harness = "claude"`, but sbxm never read it, so changing it silently did nothing [11]. `config init` no longer writes it, and a `config.toml` that still has it fails to load with `default_harness in <path> isn't an sbxm setting (use --harness; the default is claude); delete that line`. `--harness` stays the only way to pick a harness.
77. **Unknown keys in `config.toml` (after slice 21, confirmed by the user 2026-09-27):** as for profiles [51], unknown keys in the global `config.toml` (top level and `[resources]`) are errors naming the key and the file, so a typo such as `default_profle` never silently falls back to a default [11]. `default_harness` keeps its own message [76].
78. **Skills store and Pi (milestone 1 review Q1, confirmed by the user 2026-09-27):** extends [72] to Pi: the `sbx` skills store serves neither Gemini CLI nor Pi [46], so `new`/`open` warn for a Pi sandbox whenever `skills.store` isn't `off`. Issue #8.
79. **GitHub repo and review issues (confirmed by the user 2026-09-27):** the code lives in the repo `tutros/sbxm` (`origin`, default branch `main`); the history was scanned for secrets before the first push. Made **public** on 2026-09-27 at the user's request, licensed `MIT OR Apache-2.0` (`LICENSE-MIT`, `LICENSE-APACHE`); commit author emails and some local paths in older docs are public with it. Every code review finding becomes a GitHub issue labeled `must-fix`, `should-fix` or `question`, linking the lines at the reviewed commit and its section of the review file; fix commits close them with `Fixes #n`. The review file under `sdlc/reviews/` stays the full evidence record. Refined 2026-09-27 (user): issues follow a fixed template with **acceptance criteria** (observable, checkable by someone else) that define when the work is done; the code review skill checks for a GitHub remote and a logged-in `gh` before filing, and without access (e.g. inside a sandbox) records the findings as pending instead of setting anything up; the implementation skill works an issue from its criteria and closes it only with evidence for each.

## Milestone 2, first decisions (2026-09-27, before planning)

Recorded early at the user's request; planning proper starts later and may refine them.

80. **Parallel issue workers (confirmed by the user 2026-09-27):** besides comparisons, M2 covers running N worker sandboxes in parallel, each fixing one GitHub issue until its acceptance criteria [79] are met. A sandbox can't create sandboxes of its own: `sbx` runs on the host, and a route back to it would undo the isolation of [40] (nested `sbx` inside a sandbox is unverified and not planned). So orchestration stays on the host, and workers run through `sbx exec` like comparison contestants (S5). Claude Code subagents inside one sandbox remain possible but aren't isolated from each other.
81. **Host hands out issues (confirmed by the user 2026-09-27):** a host-side dispatcher picks the issues, not the workers. It lists open issues, drops any whose **Depends on** issues [79] are still open, orders the rest by label (`must-fix`, `should-fix`, `question`), and gives each worker a different issue number in its prompt. Issues linked as editing the same code (the review skill's `Related` links) never run at the same time. Only one process chooses, so there's no claim race; workers claiming issues on GitHub themselves was rejected (no atomic claim).
82. **Worker workspaces and output (confirmed by the user 2026-09-27):** each worker has its own clone and a branch per issue (`issue-<n>`), and ends by opening a PR with `Fixes #n`; workers never push to `main` or merge. The project skills come with the clone (`.claude/skills/`), so the `sbx` skills store isn't needed. Workers need the `github` secret and egress to `github.com`/`api.github.com`; without them they work from the issue text alone (implementation skill rule 5) and the host pushes.
83. **CI/CD (confirmed by the user 2026-09-27):** workers need `sbx`, so the dispatcher runs on a self-hosted runner (or scheduled task) on the user's machine; GitHub-hosted runners are assumed unable to run `sbx` (unverified). Start with a label trigger (`agent-ready` on an issue), PR checks on GitHub-hosted runners (`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`), and branch protection requiring the user's approval. Later: a nightly run for the top N unblocked issues, dispatch when an issue closes (unblocking its dependents), a manual `workflow_dispatch`, and a review worker running `sdlc-code-review` on each PR whose findings become new issues. **Security:** the repo is public, so the self-hosted runner must only be triggered by events only the user can cause (labels, schedule, manual runs, merges), never by `pull_request` from forks; workers get a fine-grained token scoped to this repo (contents, issues, pull requests).
84. **Independent review of a worker's change (confirmed by the user 2026-09-28):** a worker never reviews its own change: it shares its own assumptions, it could quietly edit what it reviews, and it tested on Linux only (on 2026-09-28 the Windows run of #1's tests was done by hand). After the worker exits, the host runs a review step before the PR is opened:
    1. **Host gates, no model:** `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` in the worker's clone on the host (Windows). A failure stops the step and is reported; no review runs.
    2. **Fresh reviewer:** a new headless session in its own sandbox (project `sbxm-review-<n>`, profile `sbxm-dev`) on its own clone of branch `issue-<n>`, so it can't change the worker's branch. It follows `sdlc-code-review` against the issue, the diff from `main` and `sdlc/decisions.md`, and writes `review.md` with evidence per finding, each ranked `must-fix`, `should-fix` or `nit`. It never commits. The host copies `review.md` next to the worker's `result.md`.
    3. **One fix round (user's choice (a)):** if there's a `must-fix` finding, the worker's sandbox gets `review.md` and fixes the findings (same TDD rules), then gates and reviewer run once more on the new commits. No further rounds.
    4. **Findings stay on the PR (user's choice (a)):** whatever is still open after the fix round goes into the PR description with `result.md`; nothing is filed as an issue automatically. This refines [83]'s "findings become new issues", which stays for reviews of merged code.
    5. **Reviewer harness (user's choice (a)):** Claude by default; the script takes `-ReviewHarness` so another harness can review once its provider's secret is stored. Only Claude's headless command is known (S5 covers the others), so other values are refused until then.
    The user still merges [83]. In `scripts/issue-workers.ps1` this is a `review -Issue n` action (blocking, gates → review → fix → re-review) run between `start` and `finish`; `finish` warns when there's no `review.md`.
85. **Codex reviews by default (confirmed by the user 2026-09-28):** supersedes [84] point 5. The reviewer is Codex with `gpt-5.6-sol` at `high` reasoning effort (Codex's own default is `low`), so it doesn't share the Claude workers' blind spots; `-ReviewHarness claude|codex` and `-ReviewModel <model>` override it per run (Claude without `-ReviewModel` uses Claude Code's default). Verified live 2026-09-28 in a throwaway sandbox (profile `sbxm-dev`, `--harness codex`): codex-cli 0.149.1 runs headless with `codex exec [-m <model>] [-c model_reasoning_effort="high"] "<prompt>"`, is preconfigured with `approval_policy = "never"` and `danger-full-access`, and reaches OpenAI through the `sbx` proxy with the user's global `openai` secret (OAuth), which sbxm's secrets check counts [58]. `codex debug models` listed `gpt-5.6-sol` (default), `gpt-5.6-terra`, `gpt-5.6-luna`, `gpt-5.5` and `gpt-5.2`. Codex reads `AGENTS.md` but not `.claude/skills/`, so the review prompt names the skill's file. Gemini and Pi reviewers still wait for S5.
86. **Every change goes through a PR and the independent reviewer (requested by the user 2026-09-28):** sbxm is used and tested while it's built. Changes made in the main session (code, docs and planning files alike) get an initial code review (`sdlc-code-review`), then go to a branch and a PR instead of straight to `main`, and the independent reviewer [84, 85] runs on that PR before the user merges. `main`'s ruleset already requires PRs; the user can bypass it as admin, but that's for emergencies.
87. **Reviewing a PR (confirmed by the user 2026-09-28):** implements [86]. `review -Pr <n>` clones the PR's branch from GitHub into `<base_dir>\sbxm-review-pr-<n>`, runs the host gates there, then the reviewer [85] in a sandbox on that clone, and posts `review.md` as a PR comment automatically (user's choice (a)). There's no fix round: the author fixes findings on the branch and runs it again. Logs and the review stay in `<base_dir>\sbxm-pr-<n>-review\`; the sandbox and clone are removed after. **Security:** the host gates run the PR's code (`cargo test`, build scripts) on the host, and the repo is public, so PRs from forks are refused (`isCrossRepository`).
88. **`scripts/issue-workers.ps1` is frozen after PR #12 (agreed with the user 2026-09-28):** the script grew from 192 to about 480 lines in one day (decisions 80–87), with no automated tests, and each independent review of PR #12 found new must-fix bugs. From now on it takes only bug fixes and the Pester tests (#13). New workflow features (fix-round policy, should-fix handling, CI triggers [83]) go to milestone 2 planning, which also decides whether this workflow moves into sbxm itself, where it would get `FakeBackend` tests and sbxm's conventions.
89. **`home_files` must be a subfolder (milestone 1 review Q2, issue #9; the user chose (a) on 2026-09-28):** a `[harness.claude] home_files` path that resolves to the folder of the file defining it (`.`, `""`, or anything that normalizes to it) is refused when the config loads, with a message asking for a subfolder. Accepting it copied the whole folder, `profile.toml` included, into the sandbox's home, and for a project's `sandbox.toml` it copied the project's metadata folder, which can include `sandbox.toml`, state and generated kits. Subfolders stay allowed. Best done alongside #3 (same path rules in `src/config.rs`) [63].
90. **Review findings become GitHub issues (requested by the user 2026-09-28; to streamline later):** printing findings only in the review's PR comment [87] isn't enough. Each should-fix (and any nit worth keeping) the reviewer reports should be opened as its own issue [79], not just listed in the comment, so nothing depends on the author reading it. PR #14's review showed the gap: a should-fix sat in a comment until it was fixed by hand. The review process needs a pass to streamline what happens when there are findings (must-fix vs should-fix handling, the fix round, issue creation). The script is frozen [88], so this goes to milestone 2 planning with the other workflow features, unless the user pulls it forward.
91. **Pester 5 tests the PowerShell scripts (issue #13; approved by the user 2026-09-28):** the repo's TDD rule covers `scripts/issue-workers.ps1` too. Framework: Pester 5 (`Install-Module Pester -MinimumVersion 5 -Scope CurrentUser`; the host has 5.5.0, plus older 5.1.1 and 3.4.0 that `Import-Module Pester -MinimumVersion 5` must skip past). Tests live in `scripts/tests/*.Tests.ps1` and run with `Invoke-Pester scripts/tests`, with `gh`, `git`, `sbx`, `sbxm` and `cargo` mocked, so they need no network, Docker, `sbx` login or GitHub. To make the script testable, its functions move to `scripts/issue-workers.psm1`; `issue-workers.ps1` keeps the parameters and the action dispatch. Script-level state (`$BaseDir`, `$Repo`, the review harness and model) is passed to the module as one config object, not read from script scope, so tests build the object they need. The move changes no behavior; decision 88's freeze still applies to everything except bug fixes and these tests. `just check` runs the Pester tests too, and `AGENTS.md` lists the command.

## Round 7: milestone 2 planning (2026-09-29)

92. **Milestone 2 scope (confirmed by the user 2026-09-29):** covers both comparisons (sdlc/idea.md's original ask) and the issue-worker migration [88][90], as one plan with two sub-milestones — **M2a** (comparisons) and **M2b** (folding `scripts/issue-workers.ps1` into sbxm) — sharing the headless-execution primitive [94] so it's built once.
93. **S5 scope (confirmed by the user 2026-09-29):** extends beyond Claude (done) to Codex and Gemini before the milestone plan is written; Pi is deferred (its `sbx exec` path is less proven and lower priority for the first comparison contestants).
94. **Shared headless-execution primitive (confirmed by the user 2026-09-29):** sbxm gets one harness-agnostic module (run to completion, capture transcript/answer/usage, enforce timeout) used by both comparison contestants and, in M2b, issue workers — replacing the PowerShell script's direct `sbx exec` calls. **User's framing, to shape the design (not scope creep — a north star, not new M2 requirements):** this primitive is expected to grow into something like a self-improving SDLC orchestrator — a "software factory" — the same way `issue-workers.ps1` kept organically growing after decision 80 (it went from 192 to ~480 lines in one day: decisions 81–91). sbxm itself is already used to build sbxm (this planning session runs inside an sbxm sandbox). Design implication to carry into the plan: the primitive's abstraction (naming, module boundaries, data model) should be a general "run an agentic task headlessly in a sandbox and capture its result," not something narrowly named/shaped around "comparison contestant," so M2b and later orchestration features (reviews, fixes, self-improvement loops) can reuse it without a rename or rewrite. This does not itself add M2 scope beyond M2a/M2b.

95. **Contestant sandboxes are throwaway (confirmed by the user 2026-09-29):** named e.g. `sbxm-run-<run-id>-<contestant-idx>`, created via `SandboxBackend` directly and removed when the run finishes, and never written to `state.json` or shown by `list`. This sidesteps M1's hash-drift/`--rebuild` semantics for runs entirely — a run always gets a fresh kit from the current merged config, there's nothing to drift from.
96. **`sbxm run <config>` is a new command family (confirmed by the user 2026-09-29):** a comparison is defined in a run-config file (contestants, task/prompt, seed, eval criteria) and launched with `sbxm run <config.toml>`, separate from the persistent-sandbox `new`/`open` family. **Addition:** a config-scaffolding helper (name TBD in the milestone plan, e.g. `sbxm run init`) writes a starter run-config, mirroring `config init`'s role for the global/profile config [43].
97. **Run results live under `.sbxm/<project>/runs/` (confirmed by the user 2026-09-29):** extends decision 40's rationale (never mounted, so no contestant sandbox can read or tamper with its own or another contestant's results) to `runs/<run-id>/<contestant>/{answer.md, diff.patch, transcript.jsonl, evals.json}` [24].
98. **Auth/budget for comparison runs (confirmed by the user 2026-09-29):** supersedes the "undecided" part of [12]. Each contestant's provider secret must already be stored in `sbx`, checked the same way [58] checks a profile's `secrets.services` before creating anything. A run-config's cost cap is applied per contestant via whatever best-effort flag that harness supports (Claude has `--max-budget-usd`, verified [S5]; Codex/Gemini equivalents are for the S5 continuation [93] to find). A harness with no budget flag gets a loud warning [11] that only the timeout enforces a limit for that contestant.

99. **Jev deferred within M2a (confirmed by the user 2026-09-29):** the first working eval pipeline ships executable checks, rubric + LLM judge, cosine similarity [23] and the human-review report, in that order; Jev [9][20] is added as a follow-up slice behind the same rubric interface [18] once that pipeline works end to end. No architecture change needed for it later — just another evaluator plus its `reqwest` HTTP client.
100. **Human review stays CLI/report-only for M2 (confirmed by the user 2026-09-29):** confirms decision 22's first half; a local web page is out of scope for M2.
101. **M2a launches with Claude, Codex and Gemini as contestant harnesses (confirmed by the user 2026-09-29):** all three together from the first working comparison slice, once S5's continuation [93] verifies Codex and Gemini's headless behavior. Pi stays out of M2a's contestant set (deferred with its spike [93]).
102. **Contestant seeding shares `src/seed.rs` (confirmed by the user 2026-09-29):** reuses `new --seed`'s directory/symlink validation and copy step [47]; a new function wraps it with `git init` plus one baseline commit (no seed history or remotes), since that stricter rule [14] is specific to comparison contestants and not part of `new --seed`'s contract.

103. **Antigravity replaces Gemini CLI as M2a's Google-model harness (confirmed by the user 2026-09-29, live spike [S5]):** refines [101]. The built-in `gemini` sbx agent grants no working Google network permissions on this machine — confirmed by `sbx policy log` denying `generativelanguage.googleapis.com` and even `oauth2.googleapis.com` (OAuth sign-in itself fails the same way), with no `google` secret stored and no way to obtain one. `sbx/antigravity-kit` (agent `agy`) was tested live instead: its kit pre-allows the Google hosts it needs via `forward-bypass` rules, and its headless mode (`agy -p "<prompt>" --output-format stream-json --dangerously-skip-permissions`) produced a complete, well-structured transcript (`init`/`step_update`/`result` events, final answer, token usage) reaching a real Google model end to end. M2a adds `antigravity` as a new `Harness` variant using the pinned-kit pattern Pi already established [73], and Gemini CLI is **excluded from M2a's contestant harnesses**. This is a live finding, not a plan-level guess — it changes [101]'s harness list from {Claude, Codex, Gemini} to {Claude, Codex, Antigravity}. **Update, same day:** the user separately got Gemini CLI authenticated in a sandbox (not reported as part of this spike's evidence trail) and it told them outright that Gemini CLI is deprecated in favor of Antigravity — so this isn't just a local egress gap, it's a dead end upstream. That makes the Antigravity switch stronger than a workaround: it's following Google's own migration. `--harness gemini`'s fate in sbxm (deprecate, remove, or leave as-is for existing users) is out of M2 planning's scope but is tracked as a pending GitHub issue [79] to file once a session with GitHub access is available.

104. **Antigravity pinned to an immutable tag (confirmed by the user 2026-09-29):** follows decision 73's pattern for Pi — the exact tag (and how it's discovered/verified) is a milestone-plan detail, not `latest`, so a run's config hash [55] means the same kit forever.
105. **No cost estimation for token-only harnesses (confirmed by the user 2026-09-29):** Codex and Antigravity report token usage but no dollar cost (unlike Claude's `total_cost_usd` [S5]); results show token usage always, and a dollar cost only when the harness reports one natively. sbxm maintains no per-model price table.
106. **No harness-neutral transcript normalization (confirmed by the user 2026-09-29):** decision 21's A/B/C anonymization uses each contestant's raw final-answer text and diff as-is (already harness-neutral per decision 20); no extra transcript-summary normalization layer across Claude/Codex/Antigravity's differently-shaped outputs [S5].
107. **Run-config is a new standalone TOML file (confirmed by the user 2026-09-29):** a comparison run (contestants, task/prompt, seed, run limits, eval rubric — decisions 18, 96) is defined in its own file (name and exact schema for the milestone plan), not merged into the profile/project layering [27][57] — a run is a one-shot task definition a user writes per comparison, not a persistent project setting.

108. **Contestant sizing reuses `GlobalConfig.resources`, with an optional per-run override (confirmed by the user 2026-09-29):** each contestant sandbox defaults to the existing global `[resources]` `cpus`/`memory` setting [43] (`src/config.rs` `Resources`, already applied to every sandbox `new` creates and already part of the config hash) — no new global config needed. The run-config file [107] may optionally override `cpus`/`memory` for that comparison alone, falling back to the global default when unset.

109. **Contestant count stays 2-4 (confirmed by the user 2026-09-29):** matches sdlc/idea.md and decision 1 exactly; enforced by M2a, not left open-ended.
110. **One `sdlc/milestone-2.md` (confirmed by the user 2026-09-29):** covers both M2a and M2b, following `sdlc/milestone-1.md`'s precedent. M2a gets full vertical-slice detail; M2b gets goal, plan-level choices and a rough slice outline, to be refined into full detail once M2a's shared headless-execution primitive [94] exists to build on.

111. **A run is standalone, not tied to a project (confirmed by the user 2026-09-29; supersedes the path in [97]):** `sbxm run <config>` needs no pre-existing `sbxm new` project. Results live under a run-scoped path not nested in any project: `<base>/.sbxm/runs/<run-id>/<contestant>/{answer.md, diff.patch, transcript.jsonl, evals.json}` [24]. Decision 97's rationale (never mounted, so no contestant sandbox can read or tamper with results) still holds — only the path's project segment is dropped, since a comparison is a one-shot task, not part of a persistent dev sandbox's lifecycle [95][107].

112. **Workspace and results split, mirroring decision 40 (confirmed by the user 2026-09-29):** refines [111]. A contestant's mounted workspace (where the agent edits files) lives at `<base>/runs/<run-id>/<contestant>/`, a plain sibling to `.sbxm/`, exactly as `<base>/<project>` is a project's mounted workspace. Results (`answer.md`, `diff.patch`, `transcript.jsonl`, `evals.json` [24]) and a copy of the run-config (including the eval rubric) live at `<base>/.sbxm/runs/<run-id>/<contestant>/` and `<base>/.sbxm/runs/<run-id>/run-config.toml`, never mounted — for the same reason decision 40 keeps a project's config out of its workspace: a contestant that could read the rubric could tailor its answer to game it, the way an agent that could edit `sandbox.toml` could widen its own egress.
113. **No git-init for unseeded contestants (confirmed by the user 2026-09-29, reverses the lean in the question that raised it):** an empty (unseeded) contestant workspace stays a plain directory; Codex's headless command passes `--skip-git-repo-check` [S5] to run there. The diff is captured with `git diff --no-index <snapshot-of-initial-state> <final-workspace>` without ever making the workspace itself a git repo. Seeded contestants are unaffected: they still get decision 14's fresh git repo and one baseline commit, and their diff is a normal `git diff` against it. Revisit git-initing empty workspaces too if a concrete advantage shows up (e.g. an agent's own `git` commands wanting a real repo).

114. **Uniform timeout enforcement (confirmed by the user 2026-09-29):** every contestant, regardless of harness, is timed out with `sbx exec <sandbox> timeout --kill-after=<grace> <limit> <headless argv>` [S5] — the one mechanism verified (twice) to actually kill the process inside the sandbox. Antigravity's own `--print-timeout` flag is not used, so there's only one enforcement code path to build and trust.
115. **Per-contestant profiles deferred (confirmed by the user 2026-09-29):** M2a's first working slice applies one profile to every contestant in a run, named at the run level. An optional per-contestant profile override (completing decision 10's "config variant") is a later slice, not part of the first vertical slice.

116. **P3 confirmed (the LLM judge runs through the shared headless primitive):** settles P3 in `sdlc/milestone-2.md`, per an independent review's recommendation (below) — no longer an open plan-level choice.
117. **External review of `sdlc/milestone-2.md` (GPT-5.5, requested by the user 2026-09-29) found six real gaps, all fixed in the plan:** each was checked against the actual code or a specific decision before being accepted.
    - **`SandboxBackend::exec`'s single `argv: &[String]` signature was too small:** confirmed against the S5 spike itself (Codex needs stdin explicitly piped; Claude's exec test used a `-w <workdir>` flag [161]) and against `eval.checks`' shell-string `command` field. Fixed: `exec(sandbox, &ExecSpec)` with `ExecSpec { argv, workdir, stdin: Closed | Piped(String) }`; a shell one-liner is wrapped by the caller as `["sh", "-c", command]`.
    - **Parallel orchestration (`std::thread::scope`) needs `SandboxBackend: Send + Sync`, which conflicts with `FakeBackend`'s `RefCell` fields:** confirmed by reading `src/backend/fake.rs` (four `RefCell` fields) and `src/backend/sbx.rs` (a stateless unit struct, already trivially `Send + Sync`). Fixed: the trait gains `Send + Sync`; `FakeBackend`'s `RefCell`s move to `Mutex`.
    - **`sbxm run init`'s planned scaffold (one contestant, others commented) would write a config invalid by [109]'s 2–4 minimum:** confirmed by re-reading the plan's own P1/schema section. Fixed: the scaffold ships with two contestants uncommented.
    - **Run results omitted the config-hash record decision 28 requires for "every sandbox and run":** confirmed by the exact wording of [28]. Fixed: a new `run.json` per run records the per-harness config hash [55], profile, kit refs and `sbx` version.
    - **Run results omitted decision 46's M2 requirement to record `sbx skills ls --json` for drift visibility:** confirmed by the exact wording of [46]'s "M2:" sentence. Fixed: `run.json` also records it, via a new `SandboxBackend::skills()` method.
    - **Writing results only after judge/cosine evaluation risked losing a contestant's raw output if evaluation failed, contradicting decision 16's "partial output kept and still evaluated":** confirmed by re-reading the plan's own slice order against [16]. Fixed: each contestant's `answer.md`/`diff.patch`/`transcript.jsonl` is written immediately when that contestant finishes, before any evaluator runs; evaluators only append to `evals.json` afterward.
    Two should-fix items were also folded in: a loud warning (pattern [11]) when a run sets `budget_usd` for a contestant whose harness has no budget flag [98], and an explicit note that `git diff --no-index` exits 1 (not an error) when it finds differences. A question about P3's timing was resolved as [116].

118. **A second external review of `sdlc/milestone-2.md` (GPT-5.5, requested by the user 2026-09-29) found one more must-fix and two more refinements, all applied:**
    - **`--repeat N` was placed in "Out of scope for M2," contradicting decision 15's "in the data model from day one; default 1":** confirmed by [15]'s exact wording. Fixed: `repeat` (default 1) is a `RunLimits` field from the first schema slice, and the orchestration loop is written as `(contestant, repeat index)` pairs from slice 6 onward — not bolted on after single runs work, and not schema-only either, so there's no later breaking change to the orchestration model or the results path. Results and mounted-workspace paths gain a repeat-index segment: `.sbxm/runs/<run-id>/<contestant>/<repeat-idx>/` and `runs/<run-id>/<contestant>/<repeat-idx>/`, refining [111][112] (with the default `repeat = 1` this is one directory, `0/`, same behavior as before repeat existed).
    - **The plan's Structure section said results are written "as soon as a contestant finishes," but slice 6 explicitly has "no results files yet" (slice 8 adds them) — an apparent conflict:** confirmed by re-reading both sections side by side. Resolved by clarifying, not by changing the build order: slices 6–7 build orchestration and workspace-prep without persistence (as originally planned), and the Structure table now says the immediate-write behavior applies "from slice 8 onward," matching the slice table exactly.
    - **`run.json` was written only after evaluation completed, so an evaluator failure could lose the run's identity/config-hash record even though per-contestant raw results already survived:** confirmed by re-reading the command-behavior table's original ordering. Fixed: `run.json` is written early, right after the run ID is generated and before any sandbox is created (identity, config hash, profile, kit refs, `sbx` version, skills snapshot, `started_at`), then updated in place with `completed_at` once everything finishes — the same "never lose what's already captured" principle [16][25] now applies to run-level metadata, not just per-contestant results.

119. **Third external review of `sdlc/milestone-2.md` (Codex gpt-5.6-sol on PR #29, 2026-09-29): four must-fix findings applied, three await the user.** Applied:
    - **Timed-out runs couldn't be represented:** `HeadlessResult` gains `status: RunStatus { Completed, TimedOut, Failed }`; exit 124/137 returns `Ok(TimedOut)` with best-effort partial answer/usage rather than an error, so [16] holds; persisted in a per-pair `result.json`; slices 1–3, 10 and 11 gain exit-124 tests.
    - **No slice generated and validated run kits [95]:** new `run::kits` module and slice 5a (kits under `.sbxm/runs/<run-id>/kits/`, effective hash including resource overrides, validation before any create, kit refs into `run.json`).
    - **The judge's secret wasn't preflighted:** the preflight and slice 4 cover `[eval.judge]`'s provider as well as contestants'.
    - **Repeat wording:** `repeat` is run-level, so the end-to-end check now repeats every contestant.
    Still open, needing the user: code-side ranking/aggregation semantics [19] (incl. repeat pairing for judge/cosine), the exact immutable Antigravity kit tag [104] (needs a live check), and how host-side Jev authenticates given `sbx` never reveals secret values.
120. **User answers to the review's three open findings (2026-09-29):**
    - **Ranking [19] is provisional (user: use the suggestion, decide after seeing real results):** `sdlc/milestone-2.md` P9 and slice 12a — 0–1 normalization, weight-normalized mean, repeats averaged, timed-out pairs scored on partial output, unscored criteria excluded, ties equal, raw scores kept for recomputation.
    - **Antigravity's tag [104] is pinned now, refreshed by a manual process, not an orchestrator (the user suggested an external orchestrator fetching the tag; pushed back and agreed):** resolving a tag at run time would change the config hash and defeat [104]. The tag is `20260928-7da9425e640d172e36abe6de3499a1c75d416c0f` (digest `sha256:dfa66e5c…`), found on Docker Hub; a warn-only `just kit-tags` recipe compares pinned versus newest for Pi and Antigravity, and re-pinning is a deliberate step (P8).
    - **Jev credentials [36]:** an MCP server was proposed and rejected (host route back into the sandbox is what [40][80] rule out, a scorer reachable from a contestant breaks [112], and an agent layer over a deterministic call adds nothing). New spike S9 decides between an `sbx`-proxy-injected secret from an evaluator sandbox and, as fallback, a host env-var token. The user's underlying idea, a protected service sandboxed agents can call for allowed outside access, is noted as a possible future direction and isn't planned.
121. **Fourth external review (Codex gpt-5.6-sol on PR #29, 2026-09-29): eleven must-fix and two should-fix findings, each checked against the code or decisions before being applied.** Verified real: Codex's stdin (slice 2 said "closed", but the spike showed `Piped("")` is required, [S5]); `src/seed.rs` copies a seed's `.git` as-is [47], so [14]'s "no history or remotes" needs an explicit delete; `deny_unknown_fields` is the config convention [11]. Applied to `sdlc/milestone-2.md`:
    - **Failures, not just timeouts, keep partial output and are evaluated** ([16] says "timeout/crash"); **the seed's `.git` is deleted before the baseline commit**, with a test.
    - **Run-config validation:** unknown keys rejected; rubric/check ids, scale levels (at least 2) and weights validated; every rejection before any write or backend call; the `Harness::unsupported` sweep runs for contestants and the judge. Antigravity's mandatory-instructions/skills gaps, if the slice-3 check finds them, produce the loud `unsupported` warning rather than a refusal (instructions are always configured, so refusing would ban the harness).
    - **Evaluator selection by table presence** (`[eval.judge]`, `[eval.cosine]`, `[eval.jev]` plus optional Jev-specific `[[eval.jev.questions]]` [20]), independently optional.
    - **Repeat pairing:** judge and cosine compare only contestants with the same repeat index, with a per-repeat A/B/C mapping; per-repeat scores are then averaged (provisional, like [120]).
    - **Identifiers:** run IDs validated for `run show` and atomically reserved on both trees; contestant directories are zero-based indices, never model text.
    - **Judge-only harnesses** get kits generated and validated in the same preflight; **per-contestant profiles [115]** get slice 12b; **M2b's outline** gains a parity inventory (status/finish/remove, PR review [87], CI triggers [83]).
    Also: `AGENTS.md`'s decision range is now 92–121.
122. **Fifth external review (Codex gpt-5.6-sol on PR #29, 2026-09-29): seven must-fix findings, six applied, one deferred, judged against the new must-fix bar (`sdlc-code-review`, PR #30).** Applied: the config hash input gains the external agent-kit ref, so a re-pin changes the hash (verified: `HashInput` in `src/config.rs` had none); the seeded diff stages everything and diffs against the recorded baseline commit ID, so agent-created files aren't dropped; repeats run as sequential waves so concurrency never exceeds the contestant count [6]; the secrets preflight includes every effective profile's `secrets.services` [58]; the judge harness gets the contestant allowlist; slice 3 gains a real Antigravity timeout/kill check [114]; the stale decision range is fixed. **Deferred as should-fix:** acceptance tests for `run show` id validation and root-collision rollback (P2), which slices 8 and 9 write test-first anyway.
123. **Sixth external review (Codex gpt-5.6-sol on PR #29, 2026-09-29, first under the new must-fix bar): four must-fix, three should-fix.** All four must-fix cited a criterion and were reproduced by the reviewer with commands; applied: an unseeded agent's own `git init` no longer pollutes `diff.patch` (`.git` excluded); executable checks run under the in-sandbox timeout wrapper so a hung check can't block persistence or teardown; a timeout is recognized from `timeout -v`'s stderr marker, not exit 137, which an OOM or `SIGKILL` crash also returns; the judge always receives each contestant's diff, per [20]. Also folded in at no cost: `--model` is always passed to Antigravity and asserted, and judge-sandbox teardown is tested. The remaining should-fix (reservation rollback test) is issue #31. No decision changes scope; if a further round finds only should-fix or nits, merge.
124. **Antigravity harness, slice 3 findings (2026-09-30, live on real `sbx` 0.43.0, `agy` 1.2.13):** (a) the pinned tag `20260928-7da9425e…` creates a sandbox; `sbx ls` reports agent `antigravity`. (b) **P5 resolved:** `agy -p` loads `~/.gemini/AGENTS.md` and `~/.gemini/GEMINI.md` (checked with a canary word in each candidate file, one at a time), and the workspace `AGENTS.md`/`GEMINI.md`; the same names in `~/.gemini/antigravity-cli/` are NOT loaded. sbxm writes mandatory instructions to `~/.gemini/AGENTS.md`. (c) **Skills store:** unverifiable without filling the store (an empty readonly store mounts nothing visible even for Claude, and sbxm never changes `sbx` global state), so Antigravity gets the `skills.store` warning like Gemini and Pi (it isn't on decision 46's served list). (d) **Timeout:** `timeout -v --kill-after` works: on TERM `agy` prints a `result` event with `status: ERROR`, `error: interrupted`, `timeout` exits 124, and no `agy` process is left (the one still listed was the interactive sign-in session). (e) **Output shape:** `{"event": "init" | "step_update" | "result", ...}`; success `result.response` is the answer with a trailing newline, `usage` has `input_tokens`, `output_tokens`, `thinking_tokens`, `cache_read_tokens`, and no cost; `agent_response` steps carry `text_delta` and per-step `usage`. Model ids come from `agy models` (for example `gemini-3.8-flash-low`); an unknown id gives a lone `result` ERROR, exit 1. (f) **RESOLVED by [137] (API key via the `google` secret works); original finding:** `agy` authenticates with a browser OAuth sign-in stored *inside the sandbox* (`~/.gemini/antigravity-cli/antigravity-oauth-token`); a fresh sandbox fails with `authentication failed or timed out` (exit 1), so throwaway run sandboxes have no login. `sbx` has a `google` secret service and injects a `GEMINI_API_KEY` placeholder, but `agy` didn't use it and no `google` secret is stored. How run sandboxes authenticate Antigravity is undecided; until then an Antigravity contestant reports `Failed(authentication failed or timed out)`. (g) `just kit-tags` (P8) works and shows Pi's pin (`20260924`) is one release behind `20260928`; re-pinning is the user's call.
125. **Run-config details settled in slice 4 (2026-09-30):** `[run]` is optional and `run.timeout` defaults to `10m` (the plan's schema showed it without a default); durations are a whole number plus `s`, `m` or `h`; a relative `task.seed` is resolved against the run-config file's folder, not the working directory; `eval.checks[].timeout` is parsed here (P6) but only used in slice 9; a `pass_fail` criterion with `levels` is an error, a `scale` needs at least 2 distinct levels; `[eval.jev]` is not accepted until slice 14 (it is an unknown key today); a contestant's `profile` is parsed and validated now although per-contestant profiles are slice 12b's feature. The provider secret per harness is `claude` -> `anthropic`, `codex` -> `openai`, `antigravity` -> `google`; the last is provisional until decision 124(f) (how Antigravity authenticates in run sandboxes) is settled, and it means an Antigravity contestant currently needs a stored `google` secret even though `agy` doesn't use one.
126. **`sbxm run` orchestration details, slice 6 (2026-09-30):** the run's two roots (`<base>/.sbxm/runs/<id>/` and `<base>/runs/<id>/`) are reserved after the checks pass and before the kits are built, and are left in place if the kits or a pair then fail (a later slice writes `run.json` there, so a failed run keeps its identity). `task.seed` is refused with "not supported yet" until slice 7. A failing, timed-out or erroring pair doesn't make the command fail: it prints one line per pair and exits 0, and the exit code stays for problems that stop the whole run (checks, kits, base dir). A sandbox that can't be removed is a `warning:` with the `sbx rm -f` command to run, and its result is kept. A non-zero exit with no usable output reports `exit code N: <stderr>` instead of the parser's "no result event" (a failure the harness itself reported, such as Codex's `turn.failed`, still wins). Real check with Claude and Codex (Antigravity can't authenticate in a fresh sandbox, 124f): two sandboxes ran in parallel and both answered.
127. **Diffs are computed against a repository the host owns, never the one in the workspace (slice 7, 2026-09-30):** [14]'s commands stay (`git add -A`, then `git diff --cached <baseline-id>`), but `git` runs with `--git-dir` pointing at a folder sbxm made outside the workspace (`<meta>/work/<contestant>/<repeat>/baseline.git`, deleted after the diff) and the workspace only as `--work-tree`, with the user's global and system git config off, `core.fsmonitor=false` and no inherited `GIT_*` variables. Reason (a security finding, verified): a contestant controls its whole workspace, including `.git/config`, and `core.fsmonitor`, `diff.external`, filter or textconv commands and hooks make plain `git status`/`add`/`diff` run arbitrary commands on the host; a control test showed un-hardened git in such a workspace running the planted command. The baseline commit is made in both repos with fixed author, date and message so they get the same id, and the workspace copy is the agent's ordinary repo (one commit, no remote). Unseeded workspaces follow [113] as written: `git diff --no-index` of an empty folder against a copy of the workspace with every `.git` and every link left out (links are listed in `Diff::skipped`, never followed, so nothing outside the workspace can enter a result), exit code 1 treated as "differences found", and the snapshot folder's name stripped from the header paths. A `.gitignore` in the seed is honoured for seeded diffs ([14]: ignored files stay excluded). The diff is captured for every pair that got a sandbox, whether its command completed, timed out or failed [16]; a pair whose seeding failed never gets a sandbox and is not asked to remove one.
128. **What a run writes to disk, slice 8 (2026-09-30):** under `<base>/.sbxm/runs/<id>/`: `run.json`, `run-config.toml` (a byte-for-byte copy of the file, rubric included), `kits/`, and per pair `<contestant-index>/<repeat-index>/` with `answer.md`, `diff.patch`, `transcript.jsonl` (all three harnesses emit NDJSON, so it is always `.jsonl`) and `result.json`. `run.json` is written after the kits are built and validated and before the first sandbox (an up-front refusal leaves no `run.json`), with `run_id`, `started_at`/`completed_at` (UTC RFC 3339; `completed_at` is `null` until the end), `sbxm_version`, `sbx_version` and `skills` (`null` plus a warning if `sbx` can't answer), `profile`, `resources` (the run's effective cpus and memory), `repeat`, `contestants[]` (index, harness, model, profile: the run's, until slice 12b) and `harnesses[]` (harness, `config_hash`, kit folders). `result.json` has `status` (`completed`, `timed_out`, `failed`, or `error` when the pair never ran), `error`, `usage` (`null` for `error`), `diff` (`ok`, `error` or `none`, plus links left out), `sandbox` and `remove_error`. The orchestrator calls a per-pair callback from the pair's own thread the moment the pair is done, and the command saves through it, so a later pair, evaluator or crash can't lose captured output [16][25]; `result.json` is written last, so its presence means the pair's other files are in place. `run.json` and `result.json` are written through a temp file and a rename. `completed_at` is set only when every pair's results were saved; a pair whose results can't be saved is a warning and leaves the run without `completed_at`. A pair that ran but failed or timed out still counts as saved. `sbxm run` ends by printing `Results: <folder>`. The plan's real check "kill one contestant mid-run" is covered by a gated fake-backend test (one pair held at exec, the other's files already on disk) instead of killing a real sandbox unattended.
129. **`sbxm run show` output, slice 9 (2026-09-30):** it prints the run's header (start and completion time, profile, `sbx` version, contestant and repeat counts), then per contestant and repeat: status (`completed (N in, M out tokens[, $cost])`, `timed out`, `failed: <why>`, `error: <why>`, or `no results saved` for a pair with no `result.json`), the answer, and a diff summary (files changed with `M`/`A`/`D`/`R` and line counts, binary files marked, links left out listed). The plan says "answers and diffs" and the command table says "a diff summary": the summary is the default and the added `--diff` flag prints the full patches, so a large diff doesn't flood the terminal. It reads only files (`run.json`, `<contestant>/<repeat>/*`), never `sbx`, and validates the id against the exact generated format before building a path from it (P2). Contestants and repeats come from `run.json`, so an interrupted run shows what is saved and marks the rest. Anonymised A/B/C labels and scores are added by the judge slices.
130. **Executable checks, slice 10 (2026-09-30):** each `[[eval.checks]]` command runs as `sh -c <command>` under `timeout -v --kill-after=10 <secs>` inside the pair's own sandbox, after the agent finished and **after the diff is captured**, so what a check builds (`target/`, caches) can't appear in the diff; the sandbox is removed afterwards as before. The check's own `timeout` (else the run's) is enforced in the sandbox [114]. Passed means exit 0; a check stopped by `timeout` (recognised by its `-v` stderr line, same rule as the agent, so a bare 137 is only a failure) is failed with `timed_out: true`; a backend error running it is failed with `error` and no exit code, and the next check still runs. Checks run for every pair that got a sandbox, whatever became of the agent's command (completed, timed out, failed, or a backend error), and not for a pair whose sandbox was never created. Verdicts go to `<contestant>/<repeat>/evals.json` under the key `checks` (id, command, passed, exit_code, timed_out, timeout_secs, error, and the last 8 KiB of stdout+stderr as `output_tail`), written by `results::merge_evals`, which keeps other evaluators' keys for the judge and cosine slices; no checks means no `evals.json`. `sbxm run` appends `; checks N/M passed` to a pair's line and `run show` lists each check (`passed id` or `failed id (exit code 1 | timed out after 45s | could not run: ...)`). A hung check delays its pair by at most its timeout plus the kill grace.
131. **The LLM judge, slice 11 (2026-09-30):** after every pair of the whole run is done, the judge runs once per repeat index, one after another, in its own throwaway sandbox `sbxm-run-<id>-judge-<repeat>` created from the judge harness's run kits [116][P3]. It compares the pairs of that index that produced output (a timed-out or failed pair is judged on what it left, its status not shown; a pair that never ran is left out; nothing to judge means no call), so answers of different repeats are never mixed [P9]. **The prompt is a file**, `<base>/runs/<id>/judge/<repeat>/judge-prompt.md` (the judge's workspace), and only a one-line pointer to it goes on the command line, because answers and diffs can exceed Windows' roughly 32 KB command-line limit. It holds the task, the rubric (id, kind, levels worst to best, notes; weights are not shown) and each candidate's answer and diff under a blind label, with answers cut at 20 KiB and diffs at 60 KiB and a note saying how much was omitted; it never contains harness, model, index or status [21]. Labels A, B, C, D are a deterministic SHA-256 shuffle of (run id, repeat, contestant index), so a run can be reproduced but no contestant is always first, and the mapping is stored per repeat in `judge/<repeat>/judge.json` and in each pair's own label. The reply is one JSON object (labels to criterion ids to `{value, reason}`, or a bare value), found in plain text, a code fence or prose; a scale value matches a level ignoring case and spaces; a missing or invalid criterion or candidate is `unscored` and listed, never 0 [P9]; no JSON object at all is an error. Values map to scores as pass_fail 0 or 1 and a scale's level index over n-1 (`eval::rubric::score`). Saved per pair under the key `judge` of `evals.json` (label, status, error, judge, criteria with value, score and reason, unscored) beside `checks`; per repeat `judge.json`, `reply.txt` and `transcript.jsonl`. A judge that can't be created, times out, fails or replies without JSON is recorded on every judged pair (label kept), printed as `Judge <harness>/<model>: repeat k/n failed: <why>` and warned about; the run still completes, and `completed_at` waits only for saving. The judge sandbox is always removed once create was asked for. `run show` prints `Judge (candidate B): <criterion> <value> (<score>)...`, then each reason, `; unscored: ...` or `not scored: <error>`, which reveals the mapping only after the fact [21]; criteria appear in alphabetical order. A judge from a contestant's provider is allowed but warned about in the preflight ("uses the same provider (anthropic) as contestants[0] (claude), ...; it may favour its own model family") [21]. **Limit:** the headless harnesses can't be run without tools, so the judge has its harness's normal tools in its empty workspace (only the prompt file); "no tools" from [P3] is not enforceable here.
132. **Scoring and ranking, slice 12a (2026-09-30), provisional [19][120]:** `eval::score` reads only what is on disk: the run's `run-config.toml` copy for the rubric and its weights (so editing that copy re-ranks a saved run) and each pair's `result.json` and `evals.json`. A pair's score is the weight-normalised mean of the criteria the judge scored (`sum(w*s) / sum(w)` over those only); a contestant's score is the mean of its pairs that have one, so a repeat the judge failed on or a pair that was never saved is left out of the average and counted in `k/n repeats scored`, never as 0; a criterion the judge skipped is listed as `unscored`. Timed-out, failed and errored pairs are included on whatever they left and counted (`N timed out`, `N failed`). Executable checks are shown as `checks passed/total` and never change the score. Contestants are ranked best first with competition ranking (1, 2, 2, 4: equal scores share a rank; scores within 1e-9 are equal, so `0.1+0.2` ties `0.3`); contestants with no score are unranked and listed last with `not scored`. The ranking prints as a `Ranking (judge scores 0-1, repeats averaged)` block at the end of `sbxm run` (before `Results:`) and under `run show`'s header, both computed from the saved files by the same code so they can't disagree; a run without a judge has no ranking. A run whose config copy can't be read loses only the ranking block in `run show` (and gets a warning from `run`).
133. **Per-contestant profiles, slice 12b (2026-09-30), completing [10] and lifting [115]'s deferral:** a contestant's optional `profile` names the profile its sandbox is built from; without it the contestant uses `[run].profile` (else `default_profile`), and so does the judge. `run::kits` builds and validates kits per (profile, harness) pair in first-use order (two contestants of one harness under two profiles get two kit sets with different hashes; a contestant naming the run's profile shares the run's set), and every profile is loaded before anything is written, so an unknown one leaves no trace; the preflight already checks each effective profile's `secrets.services` and `Harness::unsupported` warnings. Each pair's sandbox is created from its own profile's kits and its `skills.store`. The run records the profile used: `run.json` has each contestant's effective profile and a `profile` on every `harnesses[]` kit set, and each `result.json` has `profile`; `run show` marks only a contestant whose profile differs from the run's (`contestants[1] claude/b (profile strict)`). Resources (`cpus`, `memory`) stay run-wide. Real check: two Claude contestants under profiles with different `env.WHO` each read their own value inside their own sandbox.
134. **A host-side script files review findings as issues (agreed with the user 2026-09-30, after the milestone 2a review by Codex):** reviews run in sandboxes with no GitHub access (decision 79's access check), so findings stay in `sdlc/reviews/<date>-<scope>.md` marked `Issues: pending`. `scripts/file-review-issues.ps1` (with a `.psm1` and Pester tests, like the other scripts) reads such a file and files one issue per finding from the host, where `gh` works. **Where:** a new standalone script, not `issue-workers.ps1` (frozen by [88], and a different job) and not an `sbxm` subcommand yet (M2b folds the scripts into `sbxm`; the parser and renderer stay host-neutral so they move over). **Behavior:** a dry run by default and `-Create` to publish (the repo is public); idempotent through a hidden marker per finding, so a rerun never duplicates; dependency-ordered creation with ids rewritten to `#n` on both issues; the `Issues:` line of the review file rewritten afterwards; secrets refused and personal paths replaced before anything is posted; no labels created. **Format:** the skill's section 8 now defines the "Review file format" (headings per label, one `###` block per finding, the template's fields, acceptance criteria required for must-fix and should-fix); the parser also accepts Codex's real variants (`Smallest fix` for `Fix`, labels from section headings, em dashes in titles), and a finding without acceptance criteria is refused unless `-StandardCriteria` is given. The worker flow's own `review.md` format is left alone until M2b. **Review sandboxes** for a second model use the `sbxm-dev` profile (Rust toolchain, crates.io) on a fresh clone; the recipe is in the skill's "Independent reviews". The full task spec for building the script, with acceptance criteria per task, is `sdlc/specs/file-review-issues.md`. The issue's `Review:` field is the review file's path, the finding id and the reviewed commits as text, not a link, because the review file may not be committed or pushed yet (decided 2026-09-30, after the third independent review). **Existing issues are patched narrowly** (decided 2026-09-30, after the fourth independent review): once every current number is known, each issue of this review that already exists (found by its marker) has finding ids rewritten to `#n` inside its `Depends on` and `Related` lines and nowhere else, so hand edits survive; an issue that needs no change gets no `gh issue edit` call, and this also finishes the links of an interrupted run or of split `-Only` batches. A failed `gh issue list` stops the run with no writes, and finding ids are limited to `[A-Za-z0-9._-]+`. **Scrubbing is best-effort:** path and secret detection is heuristic, so read the dry run before `-Create`; further heuristic gaps are should-fix issues, not blockers. Status: built on `feat/file-review-issues` (2026-09-30); the live check on the milestone 2a review (T9) waits for the user.
135. **`--harness` is required on `open`, `stop` and `rm` (2026-09-30, issue #33), amending [70]'s default-`claude` rule for those commands.** `rm sbxm-m2-review` said "no sandbox" for a project whose only sandbox was Codex, because the silent `claude` default hid which sandbox it meant. `new` and `config show` keep the default (nothing exists yet to confuse); `rm --purge` needs no harness (it removes every one and still conflicts with `--harness`). When the named harness has no sandbox but the project has others, the error names them. (Numbered 135 to follow the numbers already used on `m2a-implementation`.)
136. **`just deploy-profiles`, and PowerShell, Pester and `just` in the `sbxm-dev` profile (2026-09-30, PR #37, from the independent review):** `scripts/deploy-profiles.ps1` copies the repo's `profiles/*` into the `profiles_dir` sbxm reads, so the repo stays the source of truth and `~/.config/sbxm/profiles` is a deployed copy (`just deploy-profiles`, rerun after a profile changes). **Target:** the script asks `sbxm config profiles-dir`, a small command that loads only the global config and prints `profiles_dir` (else `<config dir>/profiles`; an empty value prints as `.`), so the answer always follows sbxm's own parser and validation [51, 77]: an unknown key or table, a `default_harness` line or a missing config stops the deploy before anything is written, with sbxm's message. An earlier version re-implemented the TOML rules in PowerShell, and three review rounds each found another difference (escapes, quoted and dotted keys, table headers, empty values), so that approach was dropped. The command needs no profiles, projects or `sbx`, so a broken profile can't block deploying its fix; `just deploy-profiles` runs it through `cargo run`, and `-ConfigDir` is passed on as `SBXM_CONFIG_DIR`. **Semantics:** each repo profile replaces the same-named folder (files the repo dropped are removed); profiles only in `profiles_dir` are left alone; it prints new/updated/unchanged. **Failure safety:** a profile is copied to a staging folder beside the target and swapped in only when complete, so a failed copy keeps the working profile. **Tooling:** the profile installs PowerShell 7.6.6 (the Ubuntu 24.04 repo's `.deb`, since Microsoft's 26.04 repo has no `powershell` package), Pester 5.5.0 and `just` 1.58.0, all the host's versions, so sandboxes run `just script-test`; egress adds `packages.microsoft.com` and the PowerShell Gallery's index and CDN hosts. The `justfile` uses `pwsh` as its shell on every platform because `script-test` is a PowerShell command. Known gap: six `PrReview` tests need a real `sbx` to mock and fail on Linux (issue #38).
137. **Antigravity authenticates in run sandboxes with the `google` secret (spike for issue #45, 2026-10-01, sbx 0.43.0, `agy` 1.2.x), resolving [124](f) and confirming [125]:** the kit writes `~/.gemini/antigravity-cli/settings.json` as `{"modelProvider": "gemini"}` on every container start and `sbx` injects a `GEMINI_API_KEY` placeholder when a global `google` secret is stored (`sbx secret set -g google`); with that, `agy` calls the Gemini API directly and needs no browser sign-in (its changelog documents this mode; the earlier "`agy` ignores the placeholder" finding came from before the secret was stored or without the setting). **Evidence:** a fresh sandbox from the pinned kit, and a second one created by `sbxm new --harness antigravity`, each ran `agy -p "Reply with exactly: pong" --output-format stream-json --dangerously-skip-permissions` to `status: SUCCESS`, response `pong`, exit 0, with usage reported and no `antigravity-oauth-token` file. So throwaway run sandboxes need nothing but the `google` secret, which [125]'s preflight already requires; no token is copied, nothing leaves `sbx secrets` [79], and the interim idea of rejecting Antigravity contestants at preflight is dropped. **Not checked:** `--model` ids over this route beyond the default, quota limits of the key, and whether the OAuth sign-in (still how `agy` works in a sandbox without the secret) conflicts with it. (Numbered 137 to follow 135 and 136 on `main`.)
138. **Cosine (slice 12) and Jev (slice 14, spike S9) are deferred out of M2a; M2a merges to `main` after the slice 13 end-to-end check, then M2b is planned (confirmed by the user 2026-10-01).** Why: the user keeps wanting the issue-worker features that decision 88 freezes until M2b, and M2b needs only the `headless` primitive (slices 0–3, done), not the evaluators. Effect: `[eval.cosine]` stays refused (#48) and `[eval.jev]` stays an unknown key; the evaluators that ship are executable checks, the LLM judge and scoring/ranking (slices 10, 11, 12a, 12b). Cosine, Jev and S9 stay in the plan as a later milestone's scope, not dropped. The slice 13 checklist runs without cosine and Jev, with Claude, Codex and Antigravity contestants [137]. M2b planning (the `sdlc-planning` interview, refining the outline, spike S8) starts after the merge, and M2b's parity inventory already holds the wanted script features (PR #51).

## Round 8: milestone 2b planning (2026-10-01)

139. **M2b scope: the port, the three recorded wants, a task record, and review-finding filing (confirmed by the user 2026-10-01, option B):** M2b folds `scripts/issue-workers.ps1` into `sbxm` [88][92] with full parity of its actions (`start`, `status`, `review -Issue`, `review -Pr`, `finish`, `remove`), plus the three wants recorded in M2b's parity inventory (PR #51): a worker and reviewer harness choice (`claude`, `codex`, `antigravity`) recorded per worker, a base-branch option instead of the hard-coded `origin/main`, and a gates-only check. It adds a durable per-task record that every lifecycle stage can attach to, and folds `scripts/file-review-issues.ps1` [134] in (intake). Lifecycle stages without tooling today (planning, release, operate-and-learn) get explicit hooks in the record's data model, defined but not built; the long-term aim is a software factory covering each SDLC stage, with decision 94's "north star, not scope" rule still holding: M2b does not build all twelve stages. Not in M2b: CI triggers [83] unless Round 8 later pulls them in, new review policy, new features beyond this list.
140. **M2b is the PRD and spec experiment (confirmed by the user 2026-10-01):** M2b planning produces `sdlc/prd-m2b.md` (after the first interview rounds, before design) and `sdlc/spec-m2b.md` (after the spikes; the plan's Interfaces split out of `sdlc/milestone-2.md`, which keeps the work order, checklist, risks and out of scope). The `sdlc-*` skills are not edited until the experiment is judged (less duplication, fewer review findings about missing behavior); then the user decides whether to name the steps in the skills.
141. **Worker sandboxes get no GitHub access; spike S8 leaves M2b (confirmed by the user 2026-10-01):** M2b keeps the host-push model [82]: agents read the issue from a file, commit locally and write `result.md`; the host pushes, opens PRs, comments and files issues. S8 stays unrun and is no longer a prerequisite. **Nuance from the user:** at some point an orchestrator (the thing that dispatches and watches tasks, not a worker) will probably be granted GitHub access. So the design keeps "orchestrator" and "worker sandbox" as separate roles, and every GitHub call goes through the trait in [142], so granting the orchestrator access later changes who calls it, not the commands.
142. **`gh` behind a `GitHubBackend` trait (confirmed by the user 2026-10-01):** `sbxm` reaches GitHub on the host by shelling out to the `gh` CLI (the user's existing login) through a small `GitHubBackend` trait (issue list/view/edit, PR create/view/comment, issue create), with a `FakeGitHub` for tests, mirroring `SandboxBackend`/`FakeBackend`. No new dependency and no token handling in sbxm. Parsers of `gh` output are tested against captured real output, like `sbx`'s.
143. **The command group is `sbxm task` (confirmed by the user 2026-10-01):** the durable record [139] is a *task*, an issue (or PR) carried through the lifecycle. Subcommands: `start`, `status`, `review`, `finish`, `rm`, `gates`, plus finding-filing (`file-findings` or similar; name settled in the spec). Later lifecycle stages hang off the same noun. Exact flags, state layout and output formats go in `sdlc/spec-m2b.md` [140].
144. **`sbxm task start` blocks; no daemon (confirmed by the user 2026-10-01):** like `sbxm run`, the command stays until its agents finish, and several tasks run in parallel threads in one process (`--workers N`); `status` reads the task records from another terminal. `--detach` is a later addition if wanted. This fits the `headless` primitive, which blocks until the agent ends.
145. **Task layout on disk (confirmed by the user 2026-10-01):** M2a's two-root layout [126]. Metadata and results in `<base>/.sbxm/tasks/<task-id>/` (`task.json`, `issue.md`, `result.md`, `review.md`, `gates.log`, transcripts); the mounted workspace is the git clone at `<base>/tasks/<task-id>/`; the sandbox is `sbxm-task-<id>-<harness>`. The sandbox name is stored in `task.json`, not derived from the id [147].
146. **Task identity (confirmed by the user 2026-10-01):** the id is `issue-<n>`, or `pr-<n>` for a PR review; in M2b a task is one GitHub issue or PR. A task without an issue is later work. `task.json` carries a `stage` field and a free-form `hooks` section as the places later lifecycle stages attach [139].
147. **Templates before batching (confirmed by the user 2026-10-01):** M2b keeps one sandbox per task. Sharing one sandbox across several small tasks (to skip the ~2.5 min `sbxm-dev` setup) is a later experiment, and the order is fixed: pre-built sandbox templates first (they remove the setup cost without weakening isolation), batching only if still wanted. To keep the door open, the sandbox is a field of `task.json` [145]. A batch mode would be new (one sandbox, several clones, sequential), with the reviewer still in its own sandbox [84]; whether `sbx create` can mount several clones is unverified.
148. **The reviewer defaults to a different harness than the worker (confirmed by the user 2026-10-01):** keeps [85]'s reason (no shared blind spots) now that the worker's harness is configurable. Choosing the same harness for both explicitly is allowed with a warning.
149. **Findings after review (confirmed by the user 2026-10-01):** parity with [84][134]: must-fix findings trigger exactly one fix round and a re-review; everything else stays in `review.md`. Filing findings as issues is `sbxm task file-findings`, a dry run by default and `--create` to publish; nothing is filed automatically in M2b.
150. **Host gates are a configurable list of commands, run deterministically by `sbxm` (confirmed by the user 2026-10-01):** not hard-coded to cargo, so a project in another language works. The user's intent: the gates appropriate to the project's language run as a deterministic step of the build pipeline, independent of what any agent chooses to run (agents still run them inside the sandbox too, which only tests on Linux [84]). Which commands run, when, and what a failure does are settled in [151][152].
151. **Choosing the gates (confirmed by the user 2026-10-01):** an explicit list of commands in config. With no list and a `Cargo.toml` present, the default is the Rust three (`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`). With no list in any other project, `sbxm` refuses with a clear error, never a silent skip. Auto-detecting more languages (a catalogue) is later work.
152. **When gates run and what a failure does (confirmed by the user 2026-10-01):** after the worker exits, after the fix round, and on demand (`task gates`), always before the reviewer starts [84]. A failing gate marks the task `gates-failed` and stops it (parity). Feeding the gate log back to the worker as an automatic fix round is a natural next step but not in M2b.
153. **Interrupted tasks (confirmed by the user 2026-10-01):** every stage change is written to `task.json` before the work starts. `task start` on a task already in progress refuses with a hint. `task status` shows a task as "interrupted" when its recorded process is gone. `task start --restart` discards the work and starts over. No automatic resume in M2b.
154. **Issue selection is ported exactly (confirmed by the user 2026-10-01):** a pure function takes the open issues and the in-progress tasks and returns the picks plus a skip reason for every other issue. Rules as in the script [81]: order by label (must-fix, should-fix, then the rest); skip questions, issues with an open `**Depends on:**` issue, issues `**Related:**` to one in progress (checked both ways, since the line is often written on one issue only), and issues that already have a task; an explicit issue list still goes through the same skips and prints why. Wired to `GitHubBackend` [142]; unit-tested without GitHub.
155. **Repo and settings (confirmed by the user 2026-10-01):** the repo is `--repo`, else the `origin` of the current checkout. Task settings live in `sbxm-task.toml` at the target repo's root, committed with it (worker and reviewer harness and model [148], the profile, time limits, the gates list [150][151]), and flags override it. The user's global config holds only machine defaults. The profile named there replaces the hard-coded `sbxm-dev`. Unknown keys are errors, like sbxm's other configs.
156. **Prompts are files (confirmed by the user 2026-10-01):** the worker, reviewer and fix-round prompts move out of the script into `prompts/` in the sbxm repo, embedded into the binary at build time. `sbxm-task.toml` can point to a file in the target repo to override any of them.
157. **Onboarding commands and the PRD (confirmed by the user 2026-10-01):** M2b includes `sbxm task init` (writes a starter `sbxm-task.toml`, refuses to overwrite, like `run init`) and `sbxm task gates --dry-run` (prints the commands that would run). Higher-level automation of common tasks is built in layers: primitives first, then thin composed commands written as sequences of those primitives, never as new shell scripts (the script grew 192 to 480 lines in a day [88]). `sdlc/prd-m2b.md` is written; its success criteria S1-S9 are the milestone's definition of done.
158. **`task run`, and PR vs issue tasks (confirmed by the user 2026-10-01):** M2b includes `sbxm task run --issue N`, a thin composition of `start` and `review` (gates, review, the one fix round) that stops before `finish`, which needs a human. A PR review task (`pr-<n>`) and an issue task (`issue-<n>`) are separate tasks; `task.json` has a `related` field linking them.
159. **The host never runs git inside a clone the agent controls (confirmed by the user 2026-10-01):** the agent controls a clone's `.git` (hooks, `core.fsmonitor`, filters), which is how issue #41 happened for contestant diffs, and the old script ran plain git there (`status`, `finish`'s push, the reviewer's clone). Instead: the host keeps a bare repo it owns at `<base>/.sbxm/tasks/<id>/repo.git`; the worker's workspace is cloned from it; when the agent is done a fixed command inside the sandbox writes a `git bundle` into the workspace; the host verifies it and fetches it into `repo.git`. Commit counts, the push in `finish`, the reviewer's clone and the host gates' checkout all come from `repo.git`. Because `repo.git` is host-owned, network git operations on it (clone, fetch, push to GitHub) may use the user's own git configuration and credentials, unlike the hardened calls in `git.rs` that read agent-controlled repos.
160. **Gates run in two tiers (confirmed by the user 2026-10-01), refining [150][151][152]:** gates are a build-pipeline check, run by `sbxm`, never left to an agent's prompt. *Sandbox gates* (tier 1): after the agent exits, `sbxm` runs the configured commands inside the worker's sandbox (reusing M2a's in-sandbox check runner). *Host gates* (tier 2): optional Windows verification, run only after tier 1 passes and only when listed, in a clean checkout of the bundle from `repo.git`; they run agent-written code on the host, the risk decision 84 accepted, now limited to work that already passed tier 1 and documented. The worker's prompt is built from the same lists so it can self-check, but the pipeline's run is the gate. Config: `[gates] sandbox = [...]`, `host = [...]`; the Rust default is the cargo trio in the sandbox tier, host gates off unless listed; no list in a non-Rust repo is an error [151]. A failure in either tier marks the task `gates-failed` and stops it [152]. Gates run after the worker, after the fix round, and on demand.
161. **Spike S10 (run 2026-10-01 on `sbx` 0.46.0): a bundle made in a sandbox can be verified and fetched on the host, and two spec gaps found in the code.** In a throwaway sandbox on a host-initialised repo, `git switch -c`, a commit and `git bundle create .sbxm-task/branch.bundle issue-1 ^main` ran as the sandbox user with exit 0 (no "dubious ownership" problem), and the bundle was readable from the host path. On the host, `git bundle verify` said "okay" and `git -c core.hooksPath=<empty dir> fetch <bundle> refs/heads/issue-1:refs/heads/issue-1` into a bare clone gave the expected commit and `rev-list --count main..issue-1` = 1. Sandbox, repo and bare clone were removed afterwards. **Code gaps found (both fixed in M2b slice 0):** `Harness::headless_argv` always passes `--model <opts.model>` (an empty model would send `--model ""`), and Codex gets no `model_reasoning_effort=high`, which the script gave reviewers. Not yet checked: bundle behavior when the agent rewrites history or deletes the base ref (the verify step rejects what it cannot satisfy; slice 5 tests it).
162. **Risky slices are reviewed in parallel with the next slice (confirmed by the user 2026-10-01), refining the one-review-at-the-end rule (implementation rule 8):** waiting until the end of a milestone lets a flaw in an early slice be built on by every later one, gives one reviewer a very large diff, and keeps author and reviewer in the same context. The end-of-milestone sweep stays, because only it sees cross-slice inconsistencies. Risky = a security guard (trust boundary, hostile input), agent-written code run on the host, or destructive operations; the milestone plan marks them (M2b: slices 5, 7, 10). When a risky slice is committed and reported, an independent reviewer (subagent or Codex in its own sandbox) reviews only its commits in the background while the next slice is built; a must-fix becomes a TDD fix commit on the milestone branch, the rest wait for the end review. Experiment: M2b's final report compares what the slice reviews caught early with what only the final sweep caught. Applied to the `sdlc-implementation` and `sdlc-code-review` skills.

163. **Two production dependencies added in M2b, approved by the user 2026-10-02 (found by the PR 57 review, Q-1):** `regex` (the review-file parser in `src/task/findings.rs` and `file-findings`: line patterns for Scope, Issues, headings, secrets and personal paths, ported from the PowerShell script's regexes) and `sysinfo` (`src/task/record.rs`: the process start time that the `interrupted` check compares with the recorded one, so a reused pid isn't mistaken for the task's process; works on Windows and Linux). Alternatives considered: hand-written matching (the script has about twenty patterns, error-prone to port) and per-platform process calls (`wmic`/`/proc` parsing, two code paths to test). `sysinfo` is built with `default-features = false, features = ["system"]` to keep its tree small.

164. **M2b PRD/spec experiment assessed; the user's decision is pending (2026-10-02):** `sdlc/m2b-experiment-report.md` counts 39 findings across the M2b reviews against 4 in M2a's, and concludes: findings about behavior the spec names as missing or violated were few (5 of 39) and the spec worked as a review checklist; duplication did not go down (spec, README, checklist and help restate behavior, and three findings are those copies drifting); about four in five findings are failure modes and trust boundaries the spec was silent about. Options A (name the PRD/spec steps in `sdlc-planning`), B (A plus a required failure-modes/trust-boundaries section in the spec template, recommended), C (B plus README/checklist cite the spec instead of restating) and D (drop the split) are described there. The `sdlc-*` skills are not edited until the user chooses.

165. **The two task scripts are deleted (2026-10-02, the user asked for the cleanup after PR #57 merged):** `scripts/issue-workers.ps1` (+ `.psm1`) and `scripts/file-review-issues.ps1` (+ `.psm1`), with their Pester tests and fixtures, are removed; `sbxm task` and `sbxm task file-findings` replace them per the parity table in `sdlc/spec-m2b.md` section 13, and the golden cases live on in `tests/task_*.rs` with fixtures in `tests/fixtures/`. `scripts/deploy-profiles.ps1` and its test stay (decision 136), so `just script-test` now runs only those. `sandbox-issues.md` (the script walkthrough) is removed and the README's "Working on issues with `sbxm task`" section replaces it. Decisions 80-91 and 134 describe the scripts' design and stay as history; decision 88 (frozen to bug fixes) is closed by this one. Recoverable with `git log -- scripts/`. **Decision 164's freeze on the `sdlc-*` skills covers the PRD/spec methodology, not factual command and path updates (confirmed by the user 2026-10-02, PR 62 review Q-1):** command names, paths and pointers that this deletion made wrong (the `sdlc-code-review` skill now names `sbxm task file-findings` and `sbxm task review --pr`) are updated with it; the PRD/spec methodology stays frozen until the user picks among the options in `sdlc/m2b-experiment-report.md`.

166. **Cosine similarity (M2a slice 12) loads its model from local files, not from a download (implemented 2026-10-03 in issue #63):** `fastembed`'s model downloader fails on a machine whose HTTPS is intercepted (verified: HTTPS interception on this host, `InvalidCertificate(UnknownIssuer)`; the downloader uses bundled roots, not the OS store). Verified instead: `fastembed` 7.1.0 with `default-features = false, features = ["ort-download-binaries-native-tls"]` builds (156 dependency lines) and loads `Qdrant/all-MiniLM-L6-v2-onnx` from five local files in about 0.36 s with 384-dimension vectors and bit-identical repeats. So `[eval.cosine]` takes one option, `model_dir` (default `<config dir>/models/all-minilm-l6-v2/`), preflight refuses a missing file naming it and its URL, and an `Embedder` trait lets tests run with a fake. This departs from decision 23's "empty table, no options" only by that one key. Cosine is shown beside the other evals and is **not** folded into scoring and ranking (similarity measures agreement, not quality). Building `fastembed` needs egress to `cdn.pyke.io` (ONNX Runtime, sha256 pinned by `ort-sys`); verified reachable from the implementing sandbox without a profile change, so workflow note G1's egress concern turned out not to block this issue, though a production sandbox profile may still need it granted explicitly. A follow-up may add `sbxm models fetch`. Implementation issue: #63.
167. **Cosine's `evals.json` entry is keyed by contestant index, not by the judge's blind label (issue #63, refining decision 121's outline sketch of "a per-repeat A/B/C mapping" for cosine):** each pair's `evals.json["cosine"]` is `{"model": "...", "peers": {"<contestant index>": <similarity>, ...}}`, a missing/empty answer is `{"skipped": "no text answer"}`, and an embedder or model-load failure is `{"error": "<message>"}` on every would-be participant, never folded into scoring. Blind labels stay the judge's own concern (decision 21): `sbxm run`/`run show` print cosine's lines in label order only when the judge happened to score that same repeat, reading its saved `judge/<repeat>/judge.json` for display, but cosine never computes or stores labels itself, so it works with no judge configured at all.
169. **Continuing work on an open PR is one path: every finding is an issue, and an issue tied to a PR continues that PR's branch (2026-10-03, planning interview; closes G16, G17, G25; not yet implemented):** (a) Decision 79 stands without exception: every review finding, must-fix and should-fix, is filed as a GitHub issue (`task file-findings`); the PR's review comment links to them instead of being the record. (b) A finding issue carries a `must-fix` or `should-fix` label and names its PR (`PR: #n` in the body, machine-readable). (c) `task start` without `--issue` ranks open `must-fix` issues of an open PR first, then the rest as today (decision 154); the user can still name an issue. (d) A task for an issue tied to an open PR starts from that PR's branch head and records it (`task.json`: `pr`, `branch`); the sandbox is always fresh (no resume, G17), because a task is one issue and one sandbox. (e) `task finish` for such a task pushes to the PR's branch, with no extra flag (it is already the only command that pushes, decision 159), and opens no new PR; the push is refused unless the branch head still equals the recorded base (a fast-forward only, never a force). (f) Re-review is `task review --pr N` again, which is already allowed, so there is no `--again` and no fix round inside `task review --pr`; a task's own `ready` state is not re-entered (G25). (g) Rejected: `task fix --pr`, a fix round in `task review --pr`, `task start --pr` (a second path to the same work). Costs accepted: more short-lived issues, and the comment and the issues overlap. Settled the same day: labels `must-fix` and `should-fix`; the first line of a finding issue body is `PR: #n`; `file-findings` skips a finding that duplicates an open issue (same title and PR) and prints which issue already covers it. A hidden marker names one finding of one review file and matches in any state: a closed marker issue was filed and resolved, so a re-run does not file it again (the reviewer's M-1 on PR #86 asked for the opposite and was rejected by the maintainer, 2026-10-03).
170. **Built-in progress logging for `sbxm task` and `sbxm run` (2026-10-03, planning interview):** (a) Every `sbxm task` command appends its on-screen progress output to `run.log` in the task folder (`.sbxm/tasks/<id>/`), and `sbxm run` does the same in the run folder; the screen output is unchanged, the log is a copy, so redirecting output to files is no longer needed. (b) Each line has a timestamp and the stage; the `sbx` pull and install output is included. (c) Each command invocation starts with a header line: the command line, the exe path and its SHA-256, the commit if known, and the process id (closes G21 and G24; the user may later switch the hash to the version when builds change less). (d) Nothing new is logged that the screen did not show; secret values never appear (only placeholders), and a test checks that. (e) No rotation; `task rm` removes the log with the task. (f) `sbxm task logs --issue N | --pr N [--follow]` reads it. (g) Other commands (`new`, `open`, ...) do not log yet; that waits for a trigger. **Scope B, confirmed by the user 2026-10-06 (issue #87, continued by issue #129's review fixes):** the `sbxm task` portion of (a)-(e) and (g) is implemented — `src/task/runlog.rs` (`RunLog`, `Tee`) and `src/commands/task_log.rs` (the opener `main` uses); every `sbxm task` subcommand that names or starts a task tees its screen output and warnings into that task's `run.log`, including a command that fails or is refused before printing anything ordinary (the header still appears, and the final error is the one line added). Writing is best-effort: an append failure warns once (not per line) instead of silently dropping the log, and discovery (`task start`'s `--workers`) matches a task's full process identity (pid and start time, not pid alone), so a reused pid is never mistaken for an unrelated earlier task. Deferred out of this scope: (b)'s `sbx` child-process output (image pull, sandbox setup, install steps) is **not** included — it still goes only to the terminal, as `runlog.rs`'s own module comment says; folding it in is issue #114, not this one. **(f) is implemented** (issue #88, decision [189]): `sbxm task logs` also takes `--spec F`, which the issue text predates. `sbxm run`'s own `run.log` (the other half of (a)) is still not yet built.
171. **One dashboard: `sbxm status [--watch]` (2026-10-03, planning interview; Tasks section built 2026-10-12, issue #90):** (a) Form: a redrawn terminal table (plain ANSI, no new dependency), refreshed every 2 seconds (`--interval <secs>`), Ctrl+C quits; when output is not a terminal it prints once and exits. A full-screen TUI or a web page comes only if this is not enough (optimize just in time). (b) Sections: **Tasks** (id, stage, status, time in the current stage, commits ahead, must-fix findings left, PR link, last `run.log` line; a task that says "working" with no live process or sandbox is flagged `interrupted`, as `task status` already does), **Comparison runs** (run id, contestants, pairs done of total from the saved `result.json` files, time running, last log line; a pair whose sandbox is gone before it finished is flagged), **Sandboxes** (every sandbox in `sbx ls`, its owner: task, run, project or orphan; its state and age), and a **footer** with free space on the base dir's drive and the system drive, with a warning below 10 GB. (c) `sbxm task status` stays the static table of tasks, with no `--watch`: one path to the live view. (d) The dashboard only reads files and `sbx ls`; it writes nothing. (e) The Tasks section is built first; the other sections are separate issues. **The Tasks section is implemented** (issue #90, decisions 2026-10-11 ahead of it): `src/status/tasks.rs` (`rows`, pure over `task.json`/`run.log`/`repo.git`, never calling `sbx`; `render`, pure and golden-tested, taking the clock and the terminal width in) and `src/commands/status.rs` (`Env`, the `--watch` loop, plain-ANSI clear-and-home, the terminal-width probe via a raw `ioctl`/`GetConsoleScreenBufferInfo` call rather than a new dependency); `backend: &dyn SandboxBackend` is threaded through unused, for the Comparison-runs and Sandboxes sections still to come. A table wider than the terminal cuts only the last column (the log line) with `…`; a non-terminal never cuts and always prints exactly once, `--watch` or not. `--interval` needs `--watch` and is at least 1 (clap's own range check).
172. **A PR that cannot be read skips its must-fix issue; it does not stop the selection and does not start the issue (confirmed by the user 2026-10-05, review of PR #102; refines [169](c)):** when `task start --workers` cannot read the PR a `must-fix` issue names (deleted, mistyped, a network error), that issue is skipped with `its PR #n couldn't be read` and a warning carrying the lookup error, and the other issues are selected as usual. Rejected: failing the whole selection (one stale reference blocked unrelated work) and ranking the issue as an ordinary one (it could start work on a PR that is really closed). An explicit `--issue N` still starts the issue without reading its PR.

173. **The task state machine: rounds, retries, sources (2026-10-05, planning interview of 6 questions; not yet implemented; supersedes decision 116 and the one-fix-round part of 149, and replaces 169(f)'s `rm` + `review --pr` loop for stranded tasks):** (a) **Rounds:** `[worker] fix_rounds` in `sbxm-task.toml`, default 3 (user: "at least 3"); the loop stops early when a review finds 0 must-fix, and stops with a no-progress rule when a round does not lower the must-fix count (the exact rule is for the spec). (b) **Ready with must-fix left:** `task finish` opens a draft PR that lists the remaining findings, never a normal PR (needs draft support in `GitHubBackend`). (c) **Retries:** one `task resume`, which continues from the recorded stage and keeps the clone and commits; it refuses while the recorded process is alive (pid and start time, as `interrupted` does); closes T3, T5, T6, T9. `task resume --rounds N` extends the round budget of a task that used all its rounds (confirmed by the user 2026-10-05: there must always be a way to keep making progress). (d) **Failed gates (T2):** a gate failure feeds a fix round with the gate output and counts against `fix_rounds`; flaky tests (issue 104) waste rounds, so fix them first. (e) **Reviewer scope:** rounds 2 and up review only the commits since the last review (like `scope_base`), then one full review of all commits before the task is `ready`. The reviewer never fixes its own findings (independence, decision 148). (f) **The state machine is the single source:** one table of (state, event) to (next state, action) with the round counter and the task *source* in the state replaces the scattered `check_can_*` guards in `pipeline.rs`; `Stage::next/statuses/done` in `record.rs` are its start. The pipeline becomes a driver of the table, and the documentation (`task states`, a test against `sdlc/specs/task-state-machine.md`) is generated from it. Migration is in steps, beginning with a spike that builds only the table and a test that the old guards agree with it; it waits until PRs 102 and 103 merge. (g) **The task source is pluggable:** an issue, a PR, a PRD or spec file and later a local repo are sources of one task model (the goal is a software factory, see `AGENTS.md`); the state machine must not assume an issue. Not decided: how `--spec` and a local repo map to the config and the git trust boundary, which is for the spec.

174. **The task state machine's details (2026-10-06, planning interview, 5 questions; refines decision 173, not yet implemented; design in `sdlc/specs/task-state-machine.md` section 5):** (a) **No-progress rule:** a task stops when the same finding appears again after a fix attempt (the user chose this over a plain must-fix-count rule). (b) **Matching:** the reviewer marks each finding `Repeat of: <id>` or new, given the previous review in rounds 2 and up; code cross-checks that a claimed repeat shares a file path (from `Where:`, ignoring line numbers) with the earlier finding before it can stop the task. This adds one line to the review format and its parser. (c) **End state:** a stopped task ends `ready` with a recorded `stopped` reason (`repeat-finding` or `rounds-exhausted`), an additive field in `task.json`; `task finish` opens the draft PR [173(b)] and `task resume --rounds N` continues it [173(c)]. (d) **Spec tasks:** `task start --spec <file>` is a task whose source is a file (id `spec-<name>`, the text copied into the task folder as `source.md`) and which finishes without GitHub; the user chose this over a spec task that still ends in a PR, so it also brings forward the local-repo wish. A spec task's review has no PR to comment on, so `review.md` is its only output. (e) **Where a spec task's result lands:** `[finish] sink` in `sbxm-task.toml`: `local` (default; the branch stays in the host-owned `repo.git` and `finish` prints how to fetch it) or `push` (pushed to `origin`, no PR). The git trust boundary [159] is unchanged for every source. Settles the open item in 173(g).

175. **A failed preparation is retried by `task resume` (2026-10-06, from the state-table spike, `sdlc/spikes/state-table.md` finding F4; refines decision 174):** a task in `prepared / failed` (its folders, clone or sandbox could not be made) was a dead end where only `rm` worked. The user chose to add a `resume` row for it, like the retries for a failed worker, review or fix round [173(c)], so a failure before the first stage does not cost the folders already made. The same spike found that the spec table lacked the PR task's `prepared` to `reviewing` move (finding F3); section 5.2 now has it.

176. **Every PR carries a risk assessment (2026-10-06, planning interview of 6 questions, issue #123; built by PR #178, details in decision 179):** (a) **Who:** the reviewer assesses, in the review file; sbxm raises the level to a minimum from path rules but never lowers it. (b) **Levels:** low (docs, tests, or a contained change with tests), medium (a behavior change in one area, or an unresolved finding), high (security-sensitive or cross-cutting code such as the git trust boundary, secrets, egress or the state format; a change with no test; or must-fix findings left). Shown with a colored marker (🟢 🟡 🔴, because GitHub Markdown cannot color text) and the reasons, at the top of the PR description and in the review comment of a review-only PR task. (c) **Information only:** the level changes nothing sbxm does (no blocking, no automatic draft); to revisit if a high-risk change is merged without enough attention. (d) **Path minimum:** language-neutral built-in defaults (CI and workflow files, dependency manifests and lockfiles, files named like secrets) plus `[risk]` entries in `sbxm-task.toml` that add to them or replace them; this repo's own sensitive paths go in its `sbxm-task.toml`. (e) **Format:** the review file has a `Risk: low|medium|high` line right after the `Must-fix findings: <n>` line (one blank line between the two is fine, but nothing else; the parser doesn't scan further down, so a `Risk:`-looking line quoted elsewhere never counts), followed by a `## Risk` section with one bullet per reason; a missing or invalid line is "unknown" (never low) with a warning. The level is stored in `task.json`.

177. **The state machine design after a second-model review (2026-10-06; refines 173, 174, 175; not yet implemented):** Codex (`gpt-5.6-sol`) reviewed the design through `sbxm run` (run `2026-10-06-55265d`, 13 findings; the other two contestants failed to seed). The user asked for the spec to be fixed for the stuck gate failure, the resume semantics, the PR start rule, the gate no-progress rule and an idempotency design (a, b, c, d, g below); items e, f, h and i were added by the assistant from the same review and are for the user to confirm or strike in the PR. Written into `sdlc/specs/task-state-machine.md` section 5: (a) a task whose last permitted fix round leaves gates failing ends `ready` with `stopped` = `rounds-exhausted` and the gate failure recorded as unresolved, so `finish` opens a draft and `resume --rounds N` can continue it (it was stuck at `gates-failed`); (b) `resume --rounds N` is additive, N >= 1, valid from any `ready` with `stopped`, and clears `stopped`; (c) a PR task goes to gating when gates are configured and straight to reviewing when not; (d) a gate failure repeats when the same command fails again after a fix round, with its own reason `repeat-gate-failure`; (e) `Repeat of: <id>` must name an id in the immediately preceding review's findings and a shared file, else it counts as new with a warning; (f) the accepted review result (SHA-256, scope, must-fix count, findings, risk) is recorded in `task.json` when parsed, and `finish`, the no-progress check and the draft decision read that record, never `review.md` again (the saved file starts with a `Reviewer:` line, so it cannot be parsed back directly); (g) every action that leaves the task folder (push, PR, review comment, `finished`, collecting commits) records intent before and result after, and `resume` reconciles with the outside world first, so it never opens a second PR or posts a second comment; (h) a fix that times out advances to gating only once its commits are collected and verified, else it stays `fixing / failed` with the clone intact; (i) spec task ids carry a hash of the canonical path (`spec-<name>-<6 hex>`), and sources answer capability questions (`has_worker`, `runs_gates`, `review_destination`, `finish_sink`) instead of being special-cased in rows. Review findings not adopted or not yet decided: the table as an allow-list for a caller-chosen target and not a full (state, event) to (next, action) mapping is already recorded in the spike write-up and stays the migration's job (issues 115 and later); the unchecked claim that persisted illegal states such as `ready/running` need record validation is left to the implementation issues. **Second review round (Claude Opus 5.5 and Antigravity on the same design, run `2026-10-06-dac33a`; each finding below was checked against the spec text or the code before it was adopted):** (j) an abandoned gate run is never recorded as `gates-failed` (it would consume a fix round, because `abandon_interrupted_gates` sets that status today); (k) `task.json` moves to `schema` 2, `finish` checks the recorded review's must-fix count itself, `load_all` skips an unreadable record with a warning, and every read-check-write on a task holds a lock file; (l) `resume(+N)` and `stopped` apply only to sources with a worker, and a PR task's `ready` ends it; (m) the table's rows are ordered, first match wins, and a validation test asserts every reachable (state, event) pair matches exactly one row; (n) `resume` also replays the stored result of a non-running state (T4 is now closed too); (o) no-progress, refining (e): only must-fix findings count, a repeat names one of the task's open findings from any earlier review (so a narrow review cannot hide it), each round's review is its own file, and the first review after a resume ignores repeats of findings that already existed; (p) a review is `narrow` only when a `last_reviewed_commit` exists, and `open_findings` lets a stopped task's draft list everything open; (q) resume arithmetic: the round counted on entry is not added again, plain `resume` on `rounds-exhausted` needs N >= 1, and `fix_rounds` is the value recorded at start; (r) after `finish`, the way on is decision 169 (`file-findings`, then issues that continue the PR branch), not `resume`; a task that continues an open PR gets its remaining findings as a comment, not a draft; (s) a worker run that adds no commits is `working / failed`. **Proposed, for the user to confirm or strike:** `finish` with the `push` sink refuses a spec task with open findings unless `--push-unresolved`; `task rm` refuses a spec task whose result exists only in `repo.git` without `--force`; `push` never force-pushes.

178. **When a spec task's result counts as fetched (2026-10-08, user confirmed in the PR 157 review round; refines 174(e) and spec section 5.4, built by issue 143):** `task rm --spec` refuses, before asking anything, while the task's branch has commits beyond its base whose tip commit isn't in the git checkout `rm` runs from (`git cat-file -e <tip>^{commit}` there: a read in the user's own checkout, nothing runs in the agent's clone, decision 159). The refusal names the `git fetch <repo.git> <branch>:<branch>` command and `--force`, which deletes it anyway. This holds at any stage, not only `finished`. A result fetched into a different clone still needs `--force`; the user chose this over always requiring `--force` once a result has commits.

179. **The path minimum's level and matching (2026-10-09, user confirmed during issue #123's build; refines 176(d)):** (a) the built-in rules (CI and workflow files, dependency manifests and lockfiles, files named like secrets) raise the level to at least **medium**, not high: lockfiles change in many routine PRs, and a frequent high would teach the person merging to ignore it; a repo's truly sensitive paths go under `[risk] high` in its `sbxm-task.toml`. (b) `[risk]` patterns are case-insensitive **substrings** of the changed path, not globs, so no new dependency; to revisit (globs, e.g. `globset`) when a substring matches more than intended.

180. **Gates failing right after the worker hand over to `review`, not to a new fix-round path (2026-10-10, user's choice for issue #140; implemented in PR #184):** the gate run right after the worker (spec §5.1 step 5) still ends the task at `gates-failed` with no fix round, and `task start` stays a worker-only step. Instead: (a) on that failure, `task start` names `sbxm task review --issue N` / `--spec FILE` as the next step, because review already reruns the gates and feeds the failure to a fix round (`pass_gates`, `gate-failure.md`, issue 117; shown working in the issue-36 run), and it no longer suggests a manual fix in the worker's clone first; (b) `task run`, which chains start and review, continues into the review after that failure instead of stopping. Rejected: running fix rounds from the post-worker gates too (`pass_gates` there, up to `fix_rounds`): it would put fix rounds in two places and add state-table rows for a path `review` already covers. No new state or row: `gates-failed` → reviewing already exists.

181. **`sbxm run` exits non-zero when any pair ended in error, not only when every pair did (2026-10-10, proposed while preparing issue #126; the user confirmed it the same day):** everything still happens exactly as before — every pair's line, the judge, cosine, the ranking, the saved results and the `Results:` line — then, if at least one pair's `PairOutcome::result` is `Err` (sbxm could not run the contestant at all: seeding, sandbox creation or the exec itself), the run command returns an error, so the process ends like any failing command: `Error: <n> of <m> contestant runs ended in error` on stderr and exit code 1 (an error from `commands::run::run`, not a branch in `main.rs`, so the fake-backend tests cover it; PR #191 review, M-1). A contestant that ran and timed out, or whose harness failed (`RunStatus::TimedOut`, `RunStatus::Failed`), is a result, not an error, so the exit code stays 0 for those, as today; `completed_at` in `run.json` keeps its own meaning (every pair's results are on disk) and doesn't depend on the exit code. Issue #126 left "any or all" open; "any" was chosen because a script or a `task` pipeline driving `sbxm run` has to notice a comparison that is missing a contestant, which "all" would hide whenever at least one contestant still ran.

182. **`task run --spec`'s `--restart` protects an unfetched result the same way `task rm --spec` does, with no `--force` of its own (2026-10-11, judgment call while building issue 194; confirmed by the user 2026-10-11 after the PR 208 review asked, Q-1):** issue 194 asked for `--restart` to "behave as for `--issue`", but an issue task's `--restart` has nothing like decision 178's protection (an issue publishes through a GitHub PR, never only in a host-local `repo.git`). A spec task's commits land in its `repo.git` as soon as the worker makes any (whether or not the task later finishes), so discarding one on `--restart` with no check could delete work `task rm --spec` would refuse to delete. `task run --spec --restart` runs the same `check_spec_result_fetched` check `rm --spec` runs (refused unless the branch's commits are already in the checkout at `--repo-root`), before confirming or deleting anything; it has no `--force` to bypass this, so the fix is `git fetch` the branch, or `sbxm task rm --spec FILE --force` followed by a plain `task run --spec FILE`. Rejected: skipping the check (true parity with `--issue`, but risks losing a worked task's only copy); adding a `--force` flag to `run` (more surface for a case the two-command fallback already covers).

183. **The `sbxm-dev` profile no longer installs PowerShell, Pester and `just` (2026-10-10, the user agreed after a timing check of the saved run logs; changes the tooling part of [136]):** each sandbox spent about 2 min 45 s in `setup.install`, of which `cargo install just` was about 90 s and PowerShell plus Pester about 11 s, and a task builds 2 to 5 sandboxes (the worker, one reviewer per review round, one per PR review). The task gates only run `cargo fmt`, `clippy` and `test`, so none of the three was used by them. The profile keeps apt and rustup; the egress entries for `packages.microsoft.com` and the PowerShell Gallery go with the tools. **Cost:** a sandbox can no longer run `just script-test`, so a change to `scripts/` is tested on the host (issue #38, the six tests failing on Linux, loses its trigger but stays open). The rest of [136] (`just deploy-profiles`, the script and its semantics) is unchanged. A prebuilt template (spike S11) is the route to removing the remaining setup time.

184. **What `--restart` promises: refusals before the prompt, nothing after a confirmed discard (2026-10-11, the user's choice after five reviews of issue 194 (four in the task, one on PR 208) each found one more input checked after the discard; 183 is taken by the profile change of PR 204):** `task start --restart --issue` and `task run --restart` (with `--issue` or `--spec`; `task start --spec` has no `--restart`) are two steps: discard the old task after the user confirms, then start a new one. The "check before acting" rule [33, 47] covers the first step: every refusal that can be known without writing anything (flags, the task config, the repo, base and identity, the spec file, the stored secrets, an unfetched result [182]) comes before the confirmation prompt, and a refused command leaves the old task untouched. It does not cover the second step: once the user has confirmed and the old task is gone, a start that fails leaves no task, and the error says why. That includes anything that changes while the command waits at its prompt (the spec file, a stored secret) and checks that need the new task's folder (building and validating the worker's kits). This is how `--issue --restart` has behaved since M2b; the reviews of issue 194 read the rule as covering the whole command, and each fix round moved one more check in front of the discard. **Rejected for now:** making the discard undoable (move the old task aside, put it back if the start fails), which would close the gap for both paths but adds code to the delete path; it is issue #207, to be built if a restart loses a task someone wanted.

185. **`--restart`'s discard becomes undoable (2026-10-12, the user's decision before issue #207 started, recorded on the issue itself): the old task is set aside, the new one is prepared, and the old one is deleted only once that succeeded; if it fails, the old task is put back.** This supersedes decision 184's "nothing after a confirmed discard" part (the rejected idea there is what's built here); 184's "refusals before the prompt" part stands unchanged, and so does 182's unfetched-result check, both of which still run before anything is touched. Covers all three restart paths: `task run --spec --restart`, `task run --restart --issue` and `task start --restart --issue`. (a) **Sandboxes:** removed at the discard, as before (not undone) — a restored task has no sandbox, and `task resume` rebuilds a missing worker sandbox from its recorded profile ([118]). (b) **When the old task is deleted for good:** once the new task's preparation is complete (its folder, kits and sandbox exist and `task.json` is written), before the worker runs; any failure before that point puts the old task back, even if the worker or its gates fail afterward. (c) **Leftovers:** the set-aside copy sits under `<base>/.sbxm/tasks-replaced/<id>/` (same drive as the rest of the base dir, so setting aside is a rename; outside `tasks/` and `.sbxm/tasks/`, so `task status` and `task rm` never list or touch it, and no command addresses it directly). If putting it back fails, the error names both failures (why the start failed, why the restore failed) and the folder, and nothing more is deleted. A later `--restart` of the same task refuses before the prompt while a leftover copy exists, naming the folder and the two ways out (move it back by hand, or delete it). (d) **Several tasks in one command** (`task start --restart --issue 1 --issue 2`): each is settled on its own — preparation succeeded, its old copy is deleted; preparation failed, or the command never got to it (its number wasn't picked to start again), its old copy is put back. The command exits with an error if any task was put back, and its output says per task which of the three happened. Implementation: `task::finish::SetAside` (`set_aside`/`set_aside_many` move a task's folders to `tasks-replaced/<id>/` instead of deleting them; `restore`/`settle` undo or finish it, each all-or-nothing if one folder's move fails partway) plus `has_leftover`/`check_no_leftover` and `restore_after_failed_start` (the combined-error helper). (e) **Screen output can't interrupt a restart (2026-10-12, the user's choice after eight review rounds kept finding one more place where a failed screen write left an old task stranded):** the three `--restart` commands write through `finish::Unfailing`, which never fails a write, keeps the first error and drops later output; the command then runs to its end and reports the output error there (`finish::after_output`). `set_aside` checks every folder of a task before it moves any.
186. **`sbxm status` measures terminal columns with the `unicode-width` crate (2026-10-12, issue 90's review round 3; confirmed by the user 2026-10-12; 171(a)'s "no new dependency" meant no TUI framework, and this one measurement is the exception):** two hand-rolled approximations (counting characters, then a short list of East Asian wide ranges) each let one more kind of wide character (CJK, then emoji) push a row past the terminal's width, and a complete table of its own would be a few hundred lines of Unicode data to keep current. `unicode-width` 0.2 (no dependencies of its own, part of the Rust ecosystem's standard text stack) gives the full East Asian Width and emoji tables, emoji presentation sequences included. All of `src/status/tasks.rs`'s padding and cutting goes through one `display_width` built on it. The rest of 171(a) stands: plain ANSI, a raw terminal-width probe, no TUI crate. A table generated from the crate (about 650 lines plus a generator to keep current) was built in round 4 to avoid the dependency and removed again at the user's choice.
187. **The in-task review counts as the PR's review while the PR's head is the commit it covered (2026-10-11, decided with the user before issue 198 started; changes [86]):** a PR opened by `task finish` used to get `task review --pr N`, a second review of the very commit the in-task review had already passed, costing 6 to 10 minutes and a wait on every task (26 PR reviews on 2026-10-09 and 2026-10-10). Now (a) `task finish --issue N` posts the in-task review on the PR it opens, as the PR's review comment: the risk section the PR description opens with (both from `task.json`, so they always agree; no extra reviewer pass over the PR text), a line naming the issue and the reviewed commit, then `review.md`. (b) A draft PR (#120) gets its failing review posted the same way, and it counts as that head's review; a fix is a new commit, which gets `task review --pr` as usual. (c) `task review --pr N` on an already reviewed head refuses before any write, with "already reviewed at `<commit>`" and naming `--force`, which runs a review as before; any later commit on the PR (a hand fix, a rebase) is reviewed without `--force`. (d) **How it knows:** only from the issue task's own `task.json` on this machine (the PR it recorded, and the commit its review covered), compared with the PR's head from `gh pr view`; nothing read from GitHub comments decides it, since the repo is public and anyone can post a comment that looks like a review. With no task folder recording the PR (after `task rm`, or on another machine), the review runs as before. (e) **Scope:** issue tasks only. A spec task opens no PR (`push` sink) or has no remote (`local` sink), so nothing changes for either. [86] otherwise stands: every change still goes through a PR, and the independent reviewer is the in-task one for a PR `task finish` opened at the reviewed commit, `task review --pr` for everything else. **Built (issue 198):** `last_reviewed_commit` is cleared after a clean review (it only narrows the next review), so the commit is recorded as `review.commit` in `task.json`, for every issue and spec task's review; a task that continued an open PR doesn't count for (c), since its review covered only its own commits; `task finish` posts only when `review.commit` is the branch tip it pushes, and a post that fails leaves the PR open and recorded, a note in `task.json`, and an error naming `task review --pr N` (no command that posts `review.md` by hand: that file has neither the risk section nor the commit). **What counts for (c) is positive proof (the user's choice, 2026-10-12, after PR 226's two review rounds each found a case where the task record said more than GitHub had received):** `task finish` writes `review.posted`, the posted commit, into `task.json` only after GitHub accepted the comment, and `task review --pr` refuses a head on that record alone. No `review.md`, a failed post, a post that couldn't be recorded and a `task finish` interrupted before the post all leave it unset, so the PR is reviewed as before.
188. **The issue 206 process changes are adopted, as a narrow exception to the skill freeze of [140] and [164] (2026-10-12, the user's decision after the three-task experiment; the number 187 is taken by issue 198's decision on PR 226):** the user agreed on 2026-10-11 to try the four changes of issue 206 and on 2026-10-12, with `sdlc/issue-206-experiment-report.md` in hand, to adopt them although all three experiment tasks still had a must-fix finding in review round 1. Adopted: (a) the worker's self-review and the fix round that fixes the kind of problem, in the built-in worker and fix prompts and in section 9 of the `sdlc-implementation` skill; (b) the two rules merged earlier with PR 216: a finding that needs something outside the command to change while it waits at a prompt is should-fix (`sdlc-code-review`), and no open questions go to a worker (`AGENTS.md`, `sdlc-planning`); (c) issue 220 part 1, the worker limits, as prompt text only: what a worker decides, declares or hands back, in every worker, fix, gate-fix and reviewer prompt. Part 2 (sbxm support) waits until a task hands something back. **What stays as [164] left it:** the PRD/spec experiment itself. The user's decision between the two planning approaches is still pending, and beyond (a) and (b) the `sdlc-*` skills are not edited until it is made.

189. **`sbxm task logs (--issue N | --pr N | --spec FILE) [--follow]` (2026-10-12, decided with the user before issue #88 started, recorded on the issue itself; part 2 of 6 of decisions [170] and [171]):** reads a task's `run.log` ([170]'s part 1, built in #87) instead of the user opening the file by hand. (a) `--spec F` reads a spec task's log the same way (the issue was written before spec tasks existed). (b) `--follow` only stops on Ctrl+C: it keeps polling for new lines even after the command that is writing the log ends, and a missing log is not waited for, `--follow` or not (see (d)). (c) **Read-only:** `task logs` never writes to `run.log`, with or without `--follow` — no header line and no copy of its own output — so unlike every other `sbxm task` command it does not go through `task_log`'s `RunLog`/`Tee` at all; `main` calls `commands::task_logs::run` directly, writing straight to the real stdout. (d) **No log yet:** says so, naming the task's folder, and exits 1; an unknown task (no `task.json` in its folder) is refused with the `sbxm task status` hint, the same wording `task rm`/`task file-findings` already use for that case. Implementation: `src/commands/task_logs.rs`; `--follow` polls the open file handle every 500ms (`commands::task_logs::Env`, the same injectable-`sleep`-plus-test-iteration-cap shape as `commands::status::Env`'s `--watch` loop, decision [171]) rather than a filesystem watcher, since `run.log` has no rotation to react to.

190. **`task review --pr N` reviews a `ready` PR task again in place instead of refusing (2026-10-12, decided with
the user before issue #195 started, recorded on the issue itself; gap G34):** a second review round used to need two
hand steps (copy `review.md` aside, `task rm --pr N`) every time a PR's author pushed a fix, and forgetting the
copy lost the earlier round's review — the PR comment on GitHub was the only other copy. Now: (a) **An unchanged
head is refused unless `--force`.** When the PR's current head (from `gh pr view`, fetched into `repo.git`) is
still the commit the task's last review covered (`review.commit`, now also set for a PR task's own review, which it
never was before), the command refuses before writing anything, naming that commit and the `--force` command; this
is the same `--force` flag `task review --pr` already has for decision [187]'s refusal, not a second flag. (b)
**G38, the reviewer's half only:** from the second round on, the reviewer gets every earlier round's saved
`review-<n>.md` as context (`previous-review.md`, the same mechanism issue 169 built for a re-reviewed issue task),
and `review.md` opens with a "Status of earlier findings" section the reviewer writes from them: every earlier
finding (must-fix, should-fix or question) marked fixed (naming the commit), still open, or deferred to a GitHub
issue, judged from the current commits rather than the pull request's own claims. Recording the status of a hand
fix made outside a review round, and editing earlier PR comments, are not part of this: G38 stays open in
`sdlc/evals-workflow-notes.md` for that half. (c) **One new state machine row:** `ready` with status `ok`, PR tasks only, plus the `task review` event
(and the `Record::advance` move it drives) goes to `gating`: fetch the PR's new head, keep the last review's
`review-<n>.md`, run the gates (they run again, since the commits are new), then the reviewer, numbering the round
after every saved `review-<n>.md` the same way `next_review_round` already does for an issue task. A forced review
of an unchanged head takes the same row; there is no second path that skips the gates. (d) **Nothing is written
before the refusals and the fetch:** a refused attempt, or one whose fetch of the PR's head fails, leaves the task
`ready` with its `review.md` and `task.json` as they were. A PR task still has no fix round and never repeats a
finding (no `Repeat of:` field in its prompt): the no-progress rule stays issue/spec-only, since only they have a fix
round of their own to make no progress in.

191. **Every issue gets an approximate size, twice (2026-10-13, the user's decision; a second narrow exception to
the skill freeze of [140] and [164], after [188]):** (a) **At creation**, by hand or from a review finding, an issue
gets a `size:S`, `size:M` or `size:L` label and a `**Size:**` line with the reason. The sizes are judged from what
the issue shows, not from hours: S is one module with no new state or file format and no open points; M is one
command or feature with tests and docs across a few files; L changes the state machine, `task.json` or the git trust
boundary, or has three or more open points, and is a candidate for splitting. (b) **At the "Decided" step**, when the
issue's open points are answered before a worker starts (the working rule of issue 206), the size is confirmed or
changed in the issue and on the label: an issue is at its vaguest when it is filed, and the scope is only fixed
then (issue 195 grew from its second answer). (c) **Where it lives:** the table is in the `sdlc-code-review` skill,
section 8, because that is the file a sandboxed reviewer is told to follow; `AGENTS.md` has the working rule and
points to it. (d) **Not yet done in code (issue 233):** the reviewer prompts spell out their own finding format with
no size line, and `task file-findings` leaves a `**Size:**` line out of the issue it files, so until then a filed
finding gets its size by hand. (e) **Left out on purpose:** checking sizes against the time and rounds `task.json`
records, until sizes prove unreliable. The open issues were sized on 2026-10-13 by label only; each gets its reason
line at its own "Decided" step.

192. **Issue 203 closes with its report-only rules; the sandbox-starting commands move to issue 201 (2026-10-13,
both parts agreed by the user in the session that day):** (a) **Report-only `sbxm` commands** (`status`, `task states`, `task logs`, `list`, `config show`, `run show`, and `config profiles-dir`, added by PR 236's review, S-1) may run without an
approval prompt, but per machine: through a wrapper script at a fixed path, with the rules in
`.claude/settings.local.json`, because a rule in the checked-in `.claude/settings.json` cannot name a binary whose
file name changes with every build. The user adds the rules; an agent session cannot edit its own permissions.
(b) **`doctor` and `task status` stay on approval** (PR 236's review, M-1 and M-3): `doctor`'s writability check
creates and removes a file in the base dir and its kit check writes and removes kits in the temp dir, and `task
status` appends its output to the `run.log` of every task it shows [170], so neither only reads. The user had
agreed to a list with both in it; the review took them out, and `sbxm status` covers what `task status` was for.
(c) **Issue 203's item 2 is deferred, not answered:** whether `sbxm task start`, `review`, `run`, `resume` and
`gates` run without a prompt is decided with issue 201's other open points, where one command over a list makes the
question concrete; until then they stay on approval, and the user sees the exact rules before any are added.

193. **`task run` over a list of independent tasks, one after another (2026-10-13, decided with the user before
issue 201 started, one point at a time, recorded on the issue):** each evening's work was a host command per task,
each waited for and started by hand; a list cuts that to one command. (a) **A stopped task does not stop the
list.** A task that ends short of `ready` (must-fix findings left, a failed worker, a gate that could not run) is
recorded and the list goes on to the next one. The list stops only for a problem that would hit every task: `sbx`
not reachable, a missing secret, too little free disk. The tasks are independent: a later task never gets an
earlier task's work, and the docs say so. (b) **No disk check of its own.** The check is issue 210's, which was
built first; here it runs before every task of the list, not only at the start, since a task's own sandbox can fill
the drive the next one needs. A failure is a list-wide stop: the tasks done so far keep their results and the
summary names the ones not started. (c) **One task at a time.** No `--parallel`; parallel runs become their own
issue once a sequential list has proved too slow in practice. (d) **Exit code.** 0 only when every task of the list
ended `ready` (as for one task, `ready` with must-fix findings left is reported, not an error). 1 when at least one
task failed or was not started because the list stopped; the summary line per task carries the detail. (e) **No
allow rules** (the point moved from issue 203): `task start`, `review`, `run`, `resume` and `gates` stay on an
approval prompt, since one command per list leaves little to save and a rule cannot exclude `task review --pr`
(posts a comment), `--restart --yes` (discards a task) or host gates. (f) **How the list is written.** Repeated
flags, run in the order written: `sbxm task run --issue 210 --issue 202 --spec sdlc/specs/x.md`; no list file. The
other flags (`--worker-harness`, `--worker-model`, `--reviewer-harness`, `--reviewer-model`, the time limits,
`--profile`, `--base`, `--repo`) apply to every task of the list. `--restart` is refused when the list has more
than one task: there is nothing left to show the user when several tasks' old copies would be discarded at once.
Implementation note: clap's derive API carries no field for the relative order of two different repeated flags, so
`--issue`/`--spec` order is read back from the parsed command's raw `ArgMatches` (`cli::run_target_order`).

194. **A free-space check of the drive holding `sbx`'s sandbox state (2026-10-13, decided with the user before issue
210 started, one point at a time, recorded on the issue; workflow gap G8, spike S11):** a sandbox's disk file never
shrinks, one test build of this repo is 5.2 GB, and a full drive makes `sbx` answer "try: sbx login". (a)
**Threshold:** 10 GiB free by default; one key in the global `config.toml`, `min_free_disk = "10g"` (`k`, `m`, `g`,
`t`, powers of 1024), changes it, and `"0"` switches the check off. (b) **Which drive:** on Windows the drive of
`%LOCALAPPDATA%\DockerSandboxes`, the only location verified (`sbx` v0.46.0 has no setting to move it and no command
that reports it); one optional key in the global `config.toml`, `sandbox_state_dir`, overrides it on any platform. On
macOS and Linux without the key the check is skipped: `doctor` says so in one line naming the key, the other
commands run, and sbxm guesses no path. (c) **Missing or unreadable:** if the folder doesn't exist, or its free
space can't be read, `doctor` warns and the other commands run. (d) **Which commands refuse** (the builder's reading
of "a command that would create a sandbox", not a separate answer of the user's): every command that can create
one, with its other refusals and before any write: `new`, `open` when it creates or rebuilds, `run`, and `task
start`, `task review`, `task run` and `task resume`; `doctor` only warns. The refusal names the drive, the free
space and the flag. `task run` checks again before the reviewer's sandbox, since the worker can fill the drive, and
that refusal names `task review`, which continues the task. (e) **Override:** `--ignore-disk` lets one command
through. Free space is read through `doctor`'s `Host` and `disk::Guard`, so no test depends on the machine.

195. **`task rm --merged` cleans up everything sbxm made for merged tasks in one command (2026-10-13, decided with
the user, one point at a time, recorded on issue 202):** (a) **When a task counts as merged:** an issue task when
its PR is merged on GitHub; a PR-review task (`pr-N`) when PR N is merged (open, or closed without merging, is left
alone); a spec task when the last commit of its result branch is contained in the base branch of the repo it came
from, as `origin/<base>` stands after a fetch the command runs first. A branch merged by squash or rebase isn't
detected: that task is left alone and listed as "not merged", naming the plain `task rm --spec FILE` as the way to
remove it by hand. (b) **An archive before removal:** before removing a task, `task rm --merged` copies its text
records to `<base>/.sbxm/task-archive/<task-id>/` — `review.md`, every `review-<n>.md`, `result.md`, `run.log`,
`gates.log` and `task.json`, whichever exist. Never copied: transcripts, clones, `repo.git`. The summary names the
archive folder; nothing deletes or overwrites an archive (PR 241 review: a rerun after a failed removal reuses an archive holding exactly the same records, and one that differs is refused, naming the folder), and a plain `task rm` is unchanged and archives nothing. (c) **What it
never touches:** `--force` is refused together with `--merged`; a task whose result exists only in its own
`repo.git` and was never fetched [178] is left alone and listed with the reason, even if it would otherwise count
as merged. It removes only what sbxm made — sandboxes, clones and task folders; for a spec task with the `local`
sink it does not delete the worktree or the local branch in the user's own checkout, and the summary prints the two
`git` commands for each instead of running them. Without a terminal it needs `--yes`, like `task rm` today;
`--dry-run` lists and removes nothing.

196. **A retro phase skill, `sdlc-retro` (2026-10-14, the user's approval of the proposed text; a third narrow
exception to the skill freeze of [140] and [164], after [188] and [191]):** the earlier looks back (the M2b
experiment report [164], the issue 206 report [188]) were one-off; the skill makes them periodic, when the user asks,
at the end of a milestone, or once about ten PRs have merged since the last retro (an agent says so then, it doesn't
start one unasked). A retro gathers numbers only from records that hold them, proposes at most three process changes
as experiments, and changes nothing until the user decides each one; records go to `sdlc/retros/`. This adds a skill
and edits no existing `sdlc-*` skill (only `sbxm-context`'s list of phase skills names it). The freeze otherwise stands: the PRD/spec methodology choice of [164] is still the
user's, and this decision doesn't answer it.

197. **Four more `sbxm task` commands run without a prompt (2026-10-14, the user's decision in the session that
day; changes [193](e) and adds to [192]):** `task resume`, `task finish`, `task rm --pr` and `task rm --merged` are
allowed through the wrapper of [192](a), with the rules in `.claude/settings.local.json`, added by the user. Why:
waiting for an approval was part of every gap between commands (issue 202: about 2 h 16 min of machine time over
14.5 h), and these four only continue, publish or clean up work the user already started: `task finish` acts only on
a `ready` task and merging stays the user's; `task rm --pr` deletes a reviewer's copy whose review is posted on the
PR; `task rm --merged` removes only merged tasks and archives their records first [195]. Still on approval: `task
start`, `task review`, `task run` and `task gates` (they start new work or post a review), and `task rm --issue` and
`task rm --spec` (before `task finish`, a worker's result exists only in its task). A rule matches only the command
called on its own, so an agent runs these commands alone, not chained with others.
198. **`sbxm task` rewrites a worker's commit messages, and refuses a commit over 1 MiB (2026-10-15, decided with
the user, issue 244, PR 250):** before the gates, the review or any sink see them, the agent's commits are rebuilt
from a separate agent ref (`refs/sbxm/agent/<branch>`) onto the delivered branch with every closing reference and
mention neutralised by `review::defang`, deterministically and in one atomic ref move, with hooks off; only commits
the agent made are rebuilt, and signatures are dropped (the issue's "Decided" section has the rest). Why: a closing
keyword in a commit message closes that issue once the commit lands on `main`, and PRs here are merged with merge
commits. A commit object over 1 MiB (header plus message) is refused before it is read, not rewritten: the rebuild
holds several copies of a message (about 26 bytes of memory per message byte, measured in the PR 250 review), and a
highly compressible message is tiny in the bundle. Streaming the rewrite instead is deferred to issue 251, with the
trigger of a real worker commit hitting the cap. This narrows the issue's "rewrite, not refuse" for that one case.
The cap bounds sbxm's own copies, not git's: the fetch and the range walk still unpack an oversized object before the
check, at about one byte of memory per unpacked byte, as the fetch already did for every object before issue 244.
Limiting the unpacked size of a bundle's objects is deferred to issue 252 (PR 250 review round 3, M-1).

## Round 9: templates milestone planning (2026-10-15, issue 211)

Planning interview for sandboxes created from a prebuilt `sbx` template, one question at a time, from spike S11
(`sdlc/spikes/S11.md`).

199. **What goes in a template (2026-10-15, the user agreed with the recommendation):** by default a template holds
the tools only (S11's T0: the profile's install steps run once, in the build sandbox). With `--warm <repo>` it also
holds that repo's crate registry (`cargo fetch --locked`), the ONNX Runtime cache and the compiled dependencies from
one real build, with the repo's own build output removed (S11's T2, about 2.6 GB on the sandbox drive). The whole
build folder is never baked (5.2 GB, mostly rebuilt anyway). Why: only a warm template lets a sandbox run this repo's
gates with an empty allow list (the strict mode asked for on 2026-10-02), and it saves about 38 s on each sandbox's
first gate run; tools only keeps the faster start for a profile not tied to one repo. A warm template is tied to the
repo's `Cargo.lock`; what happens when the lock file changes is decided separately.

200. **Strict mode (2026-10-15, the user agreed with the recommendation):** strict is a profile setting. A sandbox
of a strict profile (a) runs no install steps: everything comes from the template; (b) gets only the profile's
runtime allow list (empty for this repo), while the build allow list is used only to build the template; (c) is
refused when its template is missing, never quietly created with installs instead. Out of scope: the Claude kit's own
`apt-get update` at startup (blocked by egress, the sandbox starts anyway, S11) and forcing cargo's offline mode (a
profile can set `CARGO_NET_OFFLINE` in its own env). Why: the user does not want workers installing unsanctioned
software or profiles' host lists creeping (2026-10-02); a quiet fallback would undo both.

201. **A template is built only on the user's command (2026-10-15, the user agreed with the recommendation):** only
`sbxm template build` builds one; sbxm never builds or rebuilds a template on its own. A missing or stale template
gives the refusal of [200] (strict) or today's installs (not strict), and the message names the exact `template
build` command. The build goes through the free-disk check of [194] like every command that creates a sandbox, and
there is no rebuild schedule. Why: a build takes minutes, opens the wide build allow list and adds about 2.6 GB on
the sandbox drive, so it stays a deliberate step.

202. **A profile opts in with a `[template]` table (2026-10-15, the user agreed with the recommendation):** the
table's presence opts the profile in; it has `strict` (default `false`: use a matching template when one exists,
else today's installs; `true`: [200]) and `allow`, the runtime allow list of sandboxes created from the template,
which is **required** (an absent list would silently mean either everything or nothing). `network.allow` keeps its
meaning; for an opted-in profile it is the build allow list. A profile without the table behaves exactly as today.
`--warm <repo>` stays a flag of `template build` [199], not a profile key, because a profile does not know the repo.

```toml
[template]
strict = true
allow = []
```

203. **When a template is stale, and what then (2026-10-15, the user agreed with the recommendation):** three
signals, each with one response. (a) The profile's build part changed (install steps, build allow list, harness kit):
the template hash in the tag no longer matches; a strict profile is refused with the `template build` command named,
a non-strict one gets today's installs and one warning line. (b) A warm template's `Cargo.lock` differs from the
workspace's `Cargo.lock` when the sandbox is created (compared by the lock file hash in sbxm's template record):
the same two responses, naming `template build --warm`. (c) The template is older than 14 days (Claude Code is
frozen at the version it was saved with): `doctor` warns, nothing refuses. Runtime-only profile changes (env, the
runtime `allow`, instructions) do not make a template stale; they go in the kit as today. The lock file check is
cargo-only (`Cargo.lock` at the workspace root) until a second package manager is needed. A worker that changes
`Cargo.lock` mid-task still fails clearly inside the sandbox; that is workflow gap G1 (per-task egress), out of
scope here.

204. **Harnesses in scope, a Pi spike, and no more Gemini work (2026-10-15, the user's decision):** templates cover
`claude` and `codex` in this milestone; `codex` gets a real-`sbx` test with `-t` as the first slice (S11 tested only
`claude`). Spike **S15** checks whether templates work with `pi`, whose `agent_arg` is a kit of its own [73]: the
user expects to use sbxm with Pi more once the main features are built, so Pi joins the milestone if S15 passes.
`antigravity` and `gemini` are refused with "templates are not supported for this harness yet". **Gemini is dropped
from future work** (the user, 2026-10-15; upstream deprecated Gemini CLI and the built-in agent had no working Google
network permissions here, [103]): no new feature supports it. Whether `--harness gemini` is removed from the code is
a separate issue, the one [103] left unfiled.

205. **Template disk use and cleanup (2026-10-15, the user agreed with the recommendation):** (a) `template build`
removes the previous template of the same profile and harness once the new one is saved, unless one of sbxm's
sandboxes or tasks still records it, in which case it stays and the build says so; (b) `template rm` refuses while
a template is in use and names the sandboxes and tasks using it (from `state.json` and every `task.json`), because
`sbx template rm --force` does not refuse (S11, question 8); (c) `doctor` lists each template with its size and age
and the total, beside the free-space line of [194]; (d) nothing else removes a template: `rm`, `task rm` and `task
rm --merged` only stop recording it as in use. Why: 1.3 to 2.6 GB per template on the drive that already ran out,
and a rebuild makes a new tag [203].

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

207. **What `template build --warm <repo>` runs, for any language (2026-10-15, plan choice P1, the user agreed with
the revised recommendation after asking how non-Rust repos would work; changes the cargo-only part of [203]):** the
repo is copied into the build workspace like a seed (no links, no `.git`, no `target`). The repo may declare its
warm-up in its `sbxm-task.toml`: `[warm] commands = [...]` (run in order with `sh -c` in the workspace, each under
the in-sandbox timeout like a gate) and `lock_files = [...]` (paths in the repo, hashed into the record). Without
`[warm]`, a repo with a `Cargo.toml` gets the built-in cargo steps: `cargo fetch --locked`, `cargo clippy
--all-targets --locked` and `cargo test --no-run --locked` with `CARGO_PROFILE_DEV_DEBUG=0
CARGO_TARGET_DIR=/tmp/target` (the default gates' environment, so sandboxes reuse the compiled dependencies), then
`cargo clean -p <pkg>` for each package `cargo metadata --no-deps` names, with `lock_files = ["Cargo.lock"]`; any
other repo is refused, naming `[warm]`. A failing step saves nothing. The staleness check of [203](b) hashes the
recorded `lock_files` instead of `Cargo.lock` only. Why: the same pattern as the task gates' cargo defaults, and it
keeps strict mode open to non-Rust repos. Limit: a template captures the home folder and `/tmp`, not the workspace
(S11, question 1), so warm-up commands must fill caches outside the repo (`~/.npm`, `~/.cache/uv`); a `node_modules`
or `.venv` built in the workspace is not saved.

208. **Config drift with templates (2026-10-15, plan choice P2, the user agreed with the recommendation):** a sandbox
records the template it was created from, or none [206]. `open`'s drift check [55] recomputes the hash of the same
variant: the baked kit for a sandbox from a template, the install kit otherwise. So a template built or removed later
is never drift. When a non-strict sandbox was created with installs and a matching template now exists, `open`
prints one hint naming `open --rebuild`; it does not refuse.

209. **The template hash and tag (2026-10-15, plan choice P3, the user agreed with the recommendation):** the
template hash covers the profile's `setup.install`, `network.allow` and `network.deny` (the build allow list), the
harness with its pinned `agent_kit` [104], and whether the template is warm; this is the hash of [203](a). The lock
file hash and the warm commands [207] are in the record, not the tag: the lock hash is checked at create time
[203](b); the warm commands are recorded but not checked, since a create cannot see the repo's `[warm]`, so changed
warm commands need a manual `template build --warm`. Tag: `sbxm-<profile>-<harness>:<first 12 hex of the hash>`
(profile names are already `[a-z0-9-]`, valid in an image name).

210. **Where a template is built (2026-10-15, plan choice P4, the user agreed with the recommendation):** the build
sandbox is `sbxm-template-<profile>-<harness>`, its workspace `<base>/.sbxm/templates/build/<profile>-<harness>/`
(never on `C:`, [56]). Both are removed after the build whether it passes or fails, and a `template build` after an
interrupted one removes their leftovers first. No agent ever runs in a build sandbox: its home folder goes into the
image, so agent sessions and transcripts would too (S11, risk 6).

211. **Every template build gets a fresh tag (2026-10-15, PR 253 review Q-1, the user agreed with the
recommendation; changes the tag format of [209]):** the tag is `sbxm-<profile>-<harness>:<hash12>-<UTC build time>`
(for example `sbxm-sbxm-dev-claude:2a9974ee6210-20261015t2210`), so a save never lands on an existing tag. Why: the
lock hash is not in the tag [209], so a warm rebuild after a lock file change would reuse the tag, and `sbx template
save` onto an existing tag replaces the image silently (S11, question 8), which would break [205]'s promise to keep a
template still in use. Refusing the rebuild while the tag is in use was rejected: any open task would block it, and
strict sandboxes would be refused meanwhile. [205]'s cleanup is unchanged; staleness still compares the hash part,
which the record holds too.

212. **A build lock tells a live build from an interrupted one (2026-10-15, PR 253 review Q-2, the user agreed with
the recommendation; adds to [210]):** `template build` writes `<base>/.sbxm/templates/<profile>-<harness>.build.json`
with its process id and start time, the identity check of a task's `interrupted` test [163], reusing that helper. A
second build while that process is alive is refused, naming the process; when the process is gone, the leftovers of
[210] are removed and the build goes on. The lock file is removed at the end whether the build passes or fails. Why:
[210]'s fixed names plus its leftover cleanup would let a second build remove a first build's live sandbox. Unique
names per build (leftovers hard to find) and refusing whenever leftovers exist (no automatic recovery) were rejected.

213. **A separate minimum `sbx` version for templates (2026-10-15, PR 253 review Q-3, the user agreed with the
recommendation):** templates need `sbx` 0.46.0, the version S11 tested, built into sbxm as a constant (not a config
key: below the tested version is unsupported). `doctor` checks it only when a profile has `[template]`; `template
build` refuses below it before writing anything; a create that would use a template treats an older `sbx` like a
missing template: refused for a strict profile [200], today's installs plus a warning otherwise [203]. The global
`min_sbx_version` (default 0.43.0, `src/config.rs`) stays for everything else, and the README says which minimum
applies to what. Why: raising the global minimum would make `doctor` fail for profiles without `[template]`,
against [202].

214. **Templates belong to the profile, not the project (2026-10-15, PR 253 review round 2 Q-4, the user agreed
with the recommendation):** `[template]` in a project's `sandbox.toml` [57] is refused. A project override that adds
a baked input (`setup.install`, `network.allow` or `network.deny`) is refused for a strict profile, naming the
override, and gets today's installs plus a warning for a non-strict one. Why: a template is built and recorded per
profile and harness, so a project variant has no template that can match it; silently dropping the project's
build inputs was rejected, and per-project templates wait for a need.

215. **Strict mode forbids the profile's install steps, not sbxm's own configuration step (2026-10-15, PR 253
review round 2 Q-5, the user agreed with the recommendation; narrows [200](a)):** a strict sandbox runs none of the
profile's `setup.install` steps (software installs), but the harness kit's root step that writes Claude's
`managed_settings` [64] (`src/kit.rs`) still runs at every create. It writes one local file and needs no egress, so
a settings change applies without a rebuild; managed settings stay out of the template hash [209]. Baking them in
was rejected: every settings change would need a template rebuild.

216. **A task checks lock files at the commit its sandbox checks out (2026-10-15, PR 253 review round 2 Q-6, the
user agreed with the recommendation; changes the base-commit rule of [206]):** issue and spec workers use their
starting commit (a resumed worker the same one); a continued PR's worker the PR head it starts from; `task review
--pr` the fetched PR head; an in-task reviewer and fix rounds the delivered branch's tip. All are read from the
task's `repo.git` [159]. A failed check refuses a strict sandbox before it is created, naming `template build
--warm`. Why: a PR that changes a lock file would otherwise pass a check made at its base and fail later in the
gates with missing-dependency errors. A worker changing a lock file mid-task stays gap G1 [203].

217. **Templates work for `codex` as they do for `claude` (2026-10-16, milestone 3 slice 0, a real-`sbx` check
on `sbx` 0.46.0 with `sbxm-ae857fa`; findings only, no design change):** a profile with one root install step
(`apt-get install -y jq`), a canary in its mandatory instructions and `secrets.services = ["openai"]` was created
with `sbxm new --harness codex` (13.7 s, the install step 7.0 s), saved with `sbx template save` once stopped (12.7
s, `flavor` `codex-docker`, 1.09 GB reported, about 0.2 GiB new on `C:`), and recreated with `sbx create -t TAG
--pull never --cpus 4 -m 8g --skills readonly --kit <common> --kit <harness-codex> codex <workspace>`. (a) With the
same kits: 8.4 s; marker files in `~` and `/tmp` came back, so the template's image was used; the kit's environment
won (`SBXM_CONFIG_HASH`); the install step ran again (3.1 s). (b) With a narrow `common` kit (no `setup.install`,
`allow: []`): 5.0 s; `jq` present; `codex exec` answered the canary from `~/.codex/AGENTS.md` in 6.4 s, so the
provider secret works from a template. (c) **The `codex` kit brings its own network rules:** `sbx policy ls
<sandbox> --wide` lists a kit rule allowing `github.com`, `api.github.com`, `codeload.github.com`,
`release-assets.githubusercontent.com` and `archive`/`security`/`ports.ubuntu.com`, whatever sbxm's kit says;
`example.com` and `crates.io` answered 403. So a `codex` sandbox's runtime allow list [202] can't be narrower than
those hosts (the `claude` kit's own startup `apt-get` was blocked in S11, so this is per harness). (d) Secrets,
presence only: no OpenAI key pattern in the environment, process command lines or filesystem (outside `/proc`,
`/sys`, `/dev`, `/run` and the workspace); `~/.codex/auth.json` holds `OPENAI_API_KEY` of length 13, a placeholder
the proxy swaps. (e) **An agent's session is captured by the template, observed:** the template sandbox held
`~/.codex/sessions/.../rollout-*.jsonl` from the `codex exec` run before the save, plus codex's state databases.
This confirms for `codex` the risk [210] guards against (no agent ever runs in a build sandbox); S11 had it inferred
for `claude`.

218. **Strict mode keeps its allow-list promise; `codex` gets strict templates only if its hosts can be restricted
(2026-10-16, PR 261 review Q-1, the user chose option (a)):** [200](b) stands: a strict sandbox gets only the
profile's runtime allow list. Because the built-in `codex` kit adds its own hosts [217](c), slice 0b of
`sdlc/milestone-3.md` first checks, with real `sbx` and bounded, whether `codex` can run with network rules limited
to `[template].allow` (for example a `codex` setup without the built-in kit's rules). If it can, sbxm uses that
setup for strict `codex` sandboxes. If it can't, a strict profile with the `codex` harness is refused with a message
naming [217](c), before any sandbox is created; non-strict `codex` templates stay supported. Rejected: amending [200]/[202] with
a documented `codex` floor of allowed hosts (option (b)), which would quietly weaken a confirmed security boundary.
The user noted that Antigravity may be similar (the right endpoints allowed might let it use the user's subscription
instead of an API key); that is a follow-up idea outside this milestone ([204]), not decided here.

219. **A risky slice's parallel review reads in parallel but builds in turn (2026-10-16, PR 263 review Q-1, the user
chose option A; refines [162]):** the independent reviewer of a risky slice still starts as soon as the slice is
committed and reads the diff while the next slice is built, but its own compile, `cargo test` and other gate runs
wait until no slice build is running, and the builder doesn't start a new build while those checks run. A reviewer
on separate capacity (not this machine) can run its checks at once. Why: two local builds of the crate at the same
time both died with no error (gap G48, 2026-10-14), and running one build at a time has worked since. Rejected:
reviewing the whole slice before the next one starts (B), which loses [162]'s early feedback without need, and
accepting parallel builds that may fail and be rerun (C).

220. **`task rm --merged` also reports the free space of `sbx`'s sandbox state drive (2026-10-16,
decided with the user, one point at a time, recorded on issue 258; workflow gap G51):** [195]'s
"freed about" line names only the base dir's drive, but a task's sandboxes take their space on the
drive `disk::state_dir` resolves (`sandbox_state_dir` or the platform default, [194]), so that line
alone said nothing about what a removal actually freed (trigger: removing the merged tasks for
issues 179 and 98 printed "freed about 0.0 GiB on drive H:" while C: gained about 20 GiB). `task rm
--merged` now reads and reports that drive too, before and after, the same way as the base dir: left
out when the folder isn't known on this platform or its free space can't be read, exactly as an
unreadable base dir already is. When the base dir and the state dir are on the same drive (the
same path, or on Windows the same drive letter), only one line is printed (the base dir's), so the
same free space isn't counted twice.

221. **`codex`'s extra hosts can be closed with per-sandbox deny rules, so strict `codex` sandboxes are possible
(2026-10-16, milestone 3 slice 0b, a real-`sbx` check on `sbx` 0.46.0; answers [218]; 220 is reserved for issue
258):** (a) The built-in `codex` kit's network rule (`sbx policy ls <sandbox> --json`: name `kit:<sandbox>`,
`provenance.created_via` `provisioned`, `editable: false`) allows the OpenAI hosts (`api.openai.com`, `openai.com`,
`auth.openai.com`, `chatgpt.com`, `files.openai.com`, `releases.openai.com`) plus `github.com`, `api.github.com`,
`codeload.github.com`, `release-assets.githubusercontent.com`, `registry.npmjs.org`, `download.docker.com` and
`archive`/`security`/`ports.ubuntu.com` (the last two hosts, npm and Docker, add to the list in [217](c)). It can't
be removed (`editable: false`), but (b) **`sbx create --deny-network HOST` (repeatable) wins over it:** with one
deny per non-OpenAI host, every one of the nine answered 403 (so did `example.com`), `api.openai.com` was still
reachable, and `codex exec` answered a canary from `~/.codex/AGENTS.md` (4.9 s, the whole command timed). The deny
rules are sandbox-scoped (`editable: true`), survive a stop and start, and are gone after `sbx rm`; the kit's startup
commands still finished (`apt-get update || true` is blocked, not fatal). (c) The same holds with `-t
docker.io/docker/sandbox-templates:codex-docker --pull never` (created in 5.0 s): the denied hosts answered 403,
while `api.github.com`, left off the deny list on purpose, answered 200. Not tried: a saved sbxm template with
sbxm's own kits (the image comes from `-t` and the network rule from the `codex` agent, so neither is expected to
change the result; end-to-end check 7 of `sdlc/milestone-3.md` covers it). **Verdict, for slice 6:** a strict `codex` sandbox is created
with one `--deny-network` per host the `codex` kit allows beyond the OpenAI hosts and the profile's
`[template].allow`, so [200](b) holds without refusing `codex`. The hosts can't be read before the sandbox exists, so the
deny list is a table in sbxm (the list in (a)). **Confirmed by the user 2026-10-16 (PR 265 review Q-1), for slice
6:** after creation and
before any agent command, sbxm reads the kit rule's hosts from `sbx policy ls <sandbox> --json` and, if one is
allowed that is neither an OpenAI host, in `[template].allow` nor denied, removes the sandbox and refuses, naming the
host, so a later `codex` kit that adds a host can't widen a strict sandbox silently.

222. **A build's kits sit next to its workspace (2026-10-17, milestone 3 slice 4, announced to the user; adds to
[210]):** `template build` writes the build sandbox's install kits to
`<base>/.sbxm/templates/build/<profile>-<harness>.kits/` and removes them with the workspace after every build. Why:
the kits are build leftovers like the workspace, so [212]'s cleanup finds both by the same fixed name.

223. **A build refuses a tag that already exists (2026-10-17, milestone 3 slice 4, announced to the user; adds to
[211]):** before any write, a build whose generated tag is already in `sbx template ls` (a second build in the same
minute) is refused: "template TAG already exists; wait a minute and build again". Why: `sbx template save` replaces
an existing tag silently (S11, question 8), and the tag has minute resolution.

224. **A failed build removes what it made, its saved tag included (2026-10-17, milestone 3 slice 4, the user
agreed):** once the kits are written, a failure removes the build sandbox (also after a failed `create`, which can
leave one), a tag saved without its record, and the build folders; a cleanup step that fails is added to the error
with the command that finishes it (`sbx rm`, `sbx template rm`, delete by hand). Why: an image no record names is
1.3-2.6 GB that nothing would ever find or remove ([205]).

225. **A previous tag stays in the record until it is gone (2026-10-17, milestone 3 slice 4, the user agreed; adds to
[205](a)):** the new record lists the old tag and every kept `previous` tag when it is written; only then are they
checked and removed. A tag in use, one that `sbx template rm` fails to remove, or one whose use can't be checked (an
unreadable `task.json`) stays in `previous` and the next build tries again; a tag whose image is already gone is
dropped silently. `template build` prints the new tag, then one line per previous tag removed or kept, with the
reason, on stdout. Why: a sandbox created during the build still finds its tag recorded, and nothing is lost to a
failed removal.

226. **An interrupted build's saved tag is not cleaned up automatically (2026-10-17, milestone 3 slice 4, the user
chose to defer):** a build killed between `sbx template save` and writing its record leaves an image no record names;
[212]'s leftover cleanup removes only the build sandbox and folders. Trigger to fix: such an image is found on this
machine (`sbx template ls` shows an `sbxm-` tag no record lists), or `template ls`/`doctor` (slices 7, 8) can list
orphans cheaply.

227. **A `new` racing a rebuild is accepted (2026-10-17, milestone 3 slice 4, the user accepted the risk):** a
sandbox creation that reads the template record just before a rebuild rewrites it, and records its tag just after the
rebuild checked for users, can lose its template to [205](a)'s removal. The window is milliseconds between two local
commands; no lock covers `new`.

228. **`template build` follows `new` on kit warnings and the base dir (2026-10-17, milestone 3 slice 4, the user
agreed):** it prints `sbx kit validate`'s warnings and refuses a base dir that doesn't exist, with `new`'s messages.

229. **`template build --warm` checks the repo before any write (2026-10-18, milestone 3 slice 5, made while the user
was away; confirmed by the user 2026-10-18):** a repo that is not a folder, has no `Cargo.toml` (until `[warm]` in slice 5b), has no
`Cargo.lock` (the `--locked` steps would fail in the sandbox anyway, and the record needs its hash) or holds a link
outside `.git` and `target` is refused before the first write. Links inside `.git` and the top `target` are neither
copied nor refused. The package names `cargo metadata` returns go into `cargo clean -p` only if they are made of
letters, digits, `-` and `_`. The warm-up steps use the default gate timeout (20 minutes each). Slice 5b: `[warm]`
needs both keys, a non-empty `commands` and `lock_files` (`[]` allowed, saying "never stale on dependencies"), each
lock file a relative path inside the repo that exists; `[warm]` replaces the cargo steps in a Rust repo too, and
`template build` reads only that table, so a repo that is not a task target can hold just `[warm]`.

230. **The config hash of a template sandbox (2026-10-18, milestone 3 slice 6, made while building, confirmed by the user 2026-10-18; adds
to [208]):** a profile with `[template]` adds a `template` part to the config hash: `strict` in both variants (it
decides whether an install sandbox may exist at all, so turning it on is drift for one), and `[template].allow` only
in a template sandbox's hash (its kit's runtime allow list; an install sandbox never sees it). A profile without
`[template]` hashes as before, so existing sandboxes show no drift. The two variants of a profile therefore have
different hashes and their kits sit in different folders. The template's tag is not hashed: a template rebuilt
later is never drift [208].

231. **A strict profile is refused with the `pi` harness for now (2026-10-19, milestone 3 slice 6 review M3S6-1, the
user agreed with the recommendation; adds to [200], [218]):** Pi's own kit allows `registry.npmjs.org` besides its
provider (spike S15, question 4), so a strict `pi` sandbox would reach a host outside `[template].allow`. `new` (and
every later command that creates from a template) refuses a strict profile with `pi` before any write, naming the
host and the fix (`strict = false`, or `claude`/`codex`). Non-strict `pi` templates stay supported. Deferred: deny
the host as [221] does for `codex`, after a bounded real-`sbx` check that Pi still works without it (issue #266,
trigger: a strict profile wanted with `pi`).

232. **The check after a strict `codex` creation fails closed (2026-10-19, milestone 3 slice 6 review Q-1, the user
agreed with the recommendation; adds to [221]):** sbxm checks every network allow rule `sbx policy ls` reports as
scoped to the sandbox (`scope: sandbox:<name>`), not only the kit's: each host must be an OpenAI host, in
`[template].allow` or denied by a rule. If no `kit:<sandbox>` allow rule is reported at all (a future `sbx` renaming
it, say), sbxm can't see what the kit allows, so it removes the sandbox and refuses too. A strict sandbox is kept
only when sbxm can see exactly what it allows; a change in `sbx` makes strict `codex` refuse until sbxm is updated.
Global rules (the user's own `sbx policy allow` for every sandbox) are left out: they are the user's deliberate
setting, outside any profile.

233. **A `new` racing `template rm` is accepted (2026-10-20, milestone 3 slice 7 review Q-1, the user accepted the
risk; adds to [227]):** `template rm` checks for users, then removes. A sandbox creation that chose the tag and
created its sandbox but hasn't written `state.json` or `task.json` yet is invisible to that check, so `rm` can remove
the tag that sandbox was just created from. The window is seconds between two local commands the user starts by
hand, and `rm` is deliberate; no lock covers sandbox creation ([227]).

234. **A remade worker sandbox is checked at the delivered branch's tip (2026-10-20, milestone 3 slice 8 review Q-2,
the user chose it; narrows [216]):** when `task resume` makes a gone worker sandbox again, the template's lock files
are compared with the delivered branch's tip, not with the worker's starting commit as [216]'s "a resumed worker the
same one" said. The two differ only when the worker had already committed; the remade sandbox mounts the clone at
that tip and also runs the fix rounds, which [216] already checks at the tip. A strict profile is refused for a
`working (failed)` rerun whose own commits changed a lock file (gap G1).

235. **A task's reviewer refused for stale lock files says how to go on (2026-10-20, milestone 3 slice 8 review Q-1,
the user chose option a; adds to [216]):** when a strict profile refuses an issue or spec task's reviewer because
the lock files at the delivered tip differ from the warm template's, the refusal keeps [216]'s `--warm` hint and adds
the command that continues the task (`sbxm task resume --issue N`, or `--spec <file>`). When the lock files at the
task's base still match the template, it also says the change came from the task's own commits (gap G1), that
`strict = false` in the profile lets the review install them instead, and the tip commit to warm a template from.
A PR task's refusal is unchanged.

236. **A lock file read from a commit is capped at 16 MiB (2026-10-21, milestone 3 slice 8 review M3S8-3, the user
chose the limit; like [198]):** `Locks::Commit` asks `git cat-file -s` for a lock file's size before reading it. One
over 16 MiB is not read and counts as differing from the warm template's, with a reason naming the file and its size:
a strict profile is refused, a non-strict one warns and installs. A PR's head is written by someone else, so its lock
file must not be read into memory whole without a bound.

237. **`sbxm run` chooses one template per profile and harness in its preflight (2026-10-21, milestone 3 slice 9,
the user agreed with the recommendation; adds to [206], [216]):** before any write, the run's preflight resolves the
template of every (profile, harness) pair a contestant or the judge uses, once, and that choice holds for every
sandbox of the pair. A warm template's lock files are checked against the seed folder, which every contestant's
workspace copies; a run without a seed has no lock files, so a warm template counts as stale (a strict profile is
refused, a non-strict one warns and installs), while a cold template is always usable. The judge follows its pair's
choice: it builds nothing, so it gets no lock check of its own.

238. **A lock file's line endings don't count (2026-10-21, milestone 3 slice 12 end-to-end step 5, the user agreed
with the recommendation; changes [207], [216]):** the lock hash reads every CRLF as LF, whether the file comes from a
folder or a commit. The end-to-end check found that a template warmed from a Windows checkout with `core.autocrlf`
(CRLF files) never matched a task, which reads the commit's LF blob, so a strict profile refused every task. A lock
file that differs only in its line endings parses the same, so the cached dependencies still fit.

239. **A running `sbxm run` is not protected from `template rm` (2026-10-22, PR 269 review round 1 Q-1, the user
chose option A; like [233]):** template users are still only `state.json` and every `task.json` [205]. A comparison
run records nothing they read, so `template rm`, or a rebuild retiring the previous tag, can remove the tag a run
chose between two of its repeat waves; the next wave's sandboxes then fail to create and are reported as not run.
Protecting runs would mean treating every `run.json` without `completed_at` as a user, which needs rules for
interrupted runs; it waits for gap G55's trigger.

240. **A harness templates don't support is an unusable template (2026-10-22, PR 269 review round 2 M-1, the user
chose the recommendation; refines [204] with [200], [203]):** for a profile with `[template]`, creating an
`antigravity` or `gemini` sandbox is decided like any other unusable template, before `sbx` is asked anything: a
strict profile is refused with "templates are not supported for the <h> harness yet; use --harness claude, codex or
pi", and a non-strict one prints that as its one warning and uses the install steps. Neither suggests `template
build`, which refuses those harnesses.

241. **Jev's design for slice 14a (2026-10-22, the user answered `sdlc/jev.md` section 5; builds on [9], [18], [20],
[36], [99], [112], [120], [138], spikes S9 and S12):** the design in `sdlc/specs/jev-evaluator-14a.md` (the draft's
proposed decision "168", never written here) is the plan, with spike S12's corrections folded in, and three answers:
(a) **the LLM judge's ranking is the headline**; Jev's ranking is printed beside it, never blended, and Jev becomes the
headline only after a calibration run shows its verdicts match the user's on existing runs; (b) **one Jev call per
contestant per repeat**: S12 saw a Noul identical over five calls and a Score vary by about 0.03 on the 0-1 scale, so
there is no averaging inside a repeat, and `--repeat` is the way to reduce noise; (c) **`model` defaults to
`jev-latest`**, every result records the model that actually answered (the response's `model`) in `evals.json` and
`run.json`, `run show` prints it, and pinning (`model = "jev-1.13.0"`) is opt-in for a fixed comparison series. From PR 280's review (Q-1, Q-2, the
user chose the recommendations): (d) a task plus answer that is over the budget on its own makes **no call**; that
pair's Jev result is an `error` naming the oversized part, never a truncated answer; (e) in a chunked pair, every chunk
after the first sends the model the first response named, and a response naming another model makes the pair an
`error`, so one pair's score always comes from one model version.
242. **No parked should-fix findings at a milestone review (2026-10-22, the first retro's proposal 1,
`sdlc/retros/2026-10-22-retro.md`; changes how [162]'s slice reviews end):** a slice review's should-fix findings
are fixed in the next slice or filed as an issue before the milestone review starts, and the milestone PR lists
each filed one with its issue number, so the review doesn't raise it again. The reviewer's half (PR 290 review M-1):
the end-of-milestone reviewer is given that list (from the slice reports, before a PR exists) and records an
unchanged listed finding under `## Already filed` instead of as a new finding; a changed or worse one is reviewed as
new, naming the issue it supersedes (`sdlc-code-review` section 1). An experiment for the next milestone;
its measure is the number of findings at the milestone review that a slice review raised first (milestone 3: 10, 3
of them as must-fix) and the milestone review's rounds (milestone 3: 3).
243. **Every retro measures the time from a stop to the next command (2026-10-22, the first retro's proposal 2):**
the `sdlc-retro` skill's gather table gets a row for it, computed from `run.log` timestamps per task with the stop's
cause and a total per cause. No code. To be tried in the next retro; all three windows of the first one had it as unknown.
244. **Boundary tests first, in place of the edge-case checklist (2026-10-22, the first retro's proposal 3):** when an
issue's open points are answered, one question replaces the edge-case checklist tried since 2026-10-14 (its tasks
had a round-1 must-fix as often as before, 3 of 6): does the change have a limit, a size, or an identity (path,
drive, ref) that decides behavior? If yes, the issue body lists the boundary cases (at the limit, one over, one
under, the largest input) and the worker writes those tests first. An experiment for the next three issues that
answer yes; its measure is their round-1 must-fix and rounds (window 3's boundary changes: #244 7 rounds, #258 5,
#285 4). In `AGENTS.md`'s "No open questions go to a worker" rule.
245. **"Run forever" is first a way of working, tried as an experiment (2026-10-23, the user's eight answers; the user
asked on 2026-10-22 to run continuously while paid tokens last; replaces the overnight list-run trial, which ran two
of its planned 2-3 nights):** (a) a Claude Code session keeps a queue and runs `task run` one task after another,
triages stops and opens PRs; the user merges; no sbxm code, and an `sbxm task loop` feature is designed only from
the trial's numbers; (b) the queue is the open issues labeled `ready-for-worker`, a label set only once an issue's
open points are answered with the user (decision 244's boundary question included) and its size is confirmed; the
user's named order wins, otherwise size first (S, then M, then L) and, within a size, the oldest issue by creation
date first (PR 292 review Q-1, the user's choice); (c) an empty queue stops the
loop with a report of the almost-ready issues, the questions blocking each and a recommended answer, gathered read-only:
no new PRs, no sandboxes, no decisions taken for the user; (d) an environment stop (free disk, `sbx` not answering,
the usage limit, a killed process, a flaky gate) has its cause cleared where possible and the task resumed once, and
is parked on a second failure; a clean task is finished into a PR; should-fix findings alone are filed as sized
issues without the label; a work stop (must-fix left, a repeat finding, rounds used up) is finished into a draft PR
listing them and goes into the report, with no hand fixes inside the loop; (e) an issue whose `Depends on:` names an
unmerged issue or PR is skipped, everything else starts from `main`, a PR a merge left in conflict is reported, not
rebased, and the loop pauses while 5 of its PRs wait for the user; (f) the loop runs only the commands of narrow
allow rules the user adds (the rule text is written for the user to review; never edited by the agent), everything
else still prompts; (g) one task at a time end to end, each worker sandbox removed once its commits are saved, a
pause and a report if `sbx`'s drive stays under `min_free_disk` after that, the usage limit waited out, no cap beyond
the plan's own; two or more tasks at a time are an experiment for after the machine's upgrade (the NVMe drive; G61);
(h) each task's issue, size, rounds, outcome, time to ready, stops by cause and duration, the user's interventions and
the loop's idle time are logged in `sdlc/batches/<date>-loop.md` (unknown where not measured); the loop stops on the
user's word, an empty queue, the cap, two environment failures in a row, or anything security-related out of the
ordinary (an unexpected host, a secret in output, an agent touching files outside its task); the trial is 3 days or
15 tasks, whichever comes first, then an assessment decides about the feature.
246. **`run.json` records each pair's template (2026-10-23, the user's answers on issue 276, PR 269 review S-7; builds
on [128], [203], [206], [237]):** every entry of `run.json`'s `harnesses` gets a `template` key: the exact tag for a
(profile, harness) pair whose sandboxes were created from a template, `null` for an install-built pair. The key is
always written, never omitted, so a reader can tell "no template" from a run older than this field; a `run.json`
without the key (written before this decision) still loads. A rebuilt template keeps its config hash but freezes a
different agent version, so the tag is part of the run's provenance. README describes it in the `sbxm run` row.

## Round 10: spike S14, Docker Cloud Sandboxes (2026-10-23, issue 245)

247. **How spike S14 runs (2026-10-23, the user's five answers):** (a) Claude runs it hands-on in the main session
between loop tasks, since it needs the host's `sbx --cloud` and the cloud's secrets (no spec for a worker). (b) A
hard cap of $10; every spike sandbox gets `--ttl 1h` and is removed after its question; the default shape except one
large shape for the build timing; the user reads the bill before and after, and Claude stops and asks before the cap.
(c) Secrets: a fake value scoped to one sandbox first, then the user's real Claude and OpenAI logins through
`sbx secret set <provider> --oauth --cloud` (the user signs in), stored only in the cloud and removed at the end unless
a cloud milestone follows. (d) Q1-Q4 (egress, secrets, timeout, bundle in and out) must pass or there is no milestone;
Q7 (Codex in the cloud, added by the user) decides whether reviews move too; Q5 (time and cost) and Q6 (templates)
inform the plan only. (e) Order: Q1, Q3, Q4, Q2, Q7, Q6, Q5; a failing must-pass question is narrowed down once, then
the spike stops. Question 7 was added to the issue on 2026-10-23.

248. **Cloud sandboxes become milestone 4 (2026-10-23, the user's decision after spike S14; builds on [56], [159],
[161], [199]-[239]):** the spike passed every must-pass question and Q7 (`sdlc/spikes/S14.md`, $0.79 spent in total), so
running sbxm's sandboxes on Docker Cloud Sandboxes is planned as its own milestone, through a planning interview like
the templates milestone's (issue 211), tracked by issue 310. Its inputs are the spike's design facts: a template
can't be combined with a kit and has its size baked in, so a cloud template sandbox is configured by `create` flags;
`exec` can't run as root, so root setup happens only through a kit at create; secrets attach at create, are scoped by
sandbox name, take literal values and set a header; global cloud secrets reach every cloud sandbox, so each one is
scoped to its sandbox; egress is refused at DNS, by connection refused or by a proxy 403; a timeout with an orphaned child ends in a stream
error rather than 137; `/tmp` is a tmpfs of half the memory; removal can need a retry; there is no workspace mount, so
code goes in and out by bundle. Until the planning starts, the user's two cloud OAuth secrets and the spike's template
(`sbxm-spike-s14-rust`) stay in the cloud.

## Round 11: the second retro (2026-10-23)

249. **A full queue before a loop window (2026-10-23, the second retro's proposal 1, `sdlc/retros/2026-10-23-retro.md`;
amends [245]'s start):** before a loop window starts, at least as many issues as the window's task count are labeled
`ready-for-worker`, M issues included where any are decided, and a merge time is agreed with the user so the cap of 5
PRs awaiting the user isn't reached while the user is away. An experiment for the next loop window; its measure is the
share of the loop's idle time (one task's `ready` to the next task's start) spent waiting for the user (the first
window: 56%, 1h57m48s of 3h30m28s) and tasks per hour of the window's span (2.6).
250. **An issue body is read back before it's labeled (2026-10-23, the second retro's proposal 2; amends [245](b)'s
labeling step):** before adding `ready-for-worker`, the issue is read back with `gh issue view` and checked for three
things: no `PR: #n` line names a merged or closed PR, the body kept its line breaks (bodies are written with the Write
tool and passed with `--body-file`), and every recorded answer agrees with the decisions it touches. An experiment for
the next 15 labeled issues; its measure is how many need a body fix or are refused before any write (the first window:
3 of 15, #222, #272, #273).
251. **Hand-made PRs get a self-review, and late commits a re-review (2026-10-23, the second retro's proposal 3):** a PR
made in the session, not by a worker, goes through the worker's self-review checklist ([188], `sdlc-implementation`)
before its independent review (`task review --pr N`); a commit pushed after a PR's last review gets a narrow re-review,
or the PR names it as unreviewed. An experiment for the next three hand-made PRs, dropped if they still need a fix
round; its measure is the hand-made PRs with a must-fix or a question in review round 1 (the second retro's window: 3
of 3, #289, #292, #311) and the commits merged after the last review (2, #304 and #311). In `AGENTS.md`'s "Workflow".

## Round 12: milestone 4 planning, cloud sandboxes (2026-10-23, issue 310)

Planning interview for running `sbxm task`'s sandboxes on Docker Cloud Sandboxes, one question at a time, from spike
S14 (`sdlc/spikes/S14.md`) and decision 248; spike S17 (`sdlc/spikes/S17.md`) ran during the interview. The plan is
`sdlc/milestone-4.md`.

252. **Milestone 4 covers `sbxm task` only (2026-10-23, the user's answer; builds on [248]):** the cloud mode is for
`sbxm task`'s sandboxes, the workers and the reviewers. `sbxm run`, `new` and `open` stay local in this milestone and
get a cloud mode later.
253. **A repo chooses the cloud, a flag overrides one command, and the task remembers (2026-10-23, the user's answer;
builds on [248]):** the per-repo default is `[sandbox] location = "cloud"` in `sbxm-task.toml`; `--cloud` and
`--local` override it for one command. The profile stays free of any location, so one profile serves both. A task
records its location in `task.json`, so `task resume`, `task finish` and `task rm` find its sandbox where it was
created, whatever the config says later.
254. **A cloud sandbox is built from the profile's kits; cloud templates come later (2026-10-23, the user's answer;
builds on [62], [199]-[206], [248]):** a cloud sandbox gets the profile's two kits at `sbx --cloud create` (S14 Q1),
and the install steps run at create. Cloud templates are deferred; the trigger is cloud setup taking over about 2
minutes a task. Grounds: S14 Q6 showed a cloud template can't be combined with a kit and has its size baked in, so a
template design needs its own work, while kits work today (24 s to ready for `sbxm-dev`'s install steps on the large
shape, S14 Q5).
255. **A strict profile in the cloud: install at create, then remove the build hosts (2026-10-23, the user's answer
B, confirmed by spike S17; builds on [200], [221], [232]):** a strict profile's cloud sandbox is created with the
build allow list so its install steps can run. Before the worker starts, sbxm removes every host
outside the runtime list from the sandbox's cloud policy and checks that `policy ls` shows only the runtime list,
like decision 232 does for a strict `codex` sandbox. In the cloud this replaces [200](a)'s "no install steps" (there
is no template to start from, [254]); [200](b), only the runtime list while the agent runs, and [200](c), never a
quiet fallback, still hold. The fallback the user named, A (refuse strict profiles in the cloud until cloud
templates, a non-strict cloud profile instead), was to apply if rules couldn't be removed from a running cloud
sandbox. Spike S17 (2026-10-23, `sbx` v0.46.0) showed they can: `policy rm network --sandbox <name> --resource
<host> -f` and `policy deny network --sandbox <name> <host>` both exit 0, print the resulting list and take effect
at once on the running sandbox, for a kit's host and for a host of the agent's own built-in list alike. So B holds
and A is not needed.
256. **Provider logins stay global; every other secret is scoped to its sandbox (2026-10-23, the user's answer;
builds on [30], [241]; refines [248]):** this refines [248]'s conclusion that "global cloud secrets reach every cloud
sandbox, so each one is scoped to its sandbox": provider OAuth logins are the exception and stay global, and every
other secret is still scoped. The provider OAuth secrets (`sbx secret set <provider> --oauth --cloud`) stay at
global scope. Their value never enters a sandbox (S14 Q2, Q7), and egress lets a sandbox reach only its harness's own
hosts. Preflight checks that the harness's provider secret exists in the cloud. Every other secret a profile names is
scoped to the sandbox's name before `create` and removed after the sandbox is gone, the pattern of Jev's scoped
secret [241]. Grounds: S14 Q7 showed global cloud secrets reach every cloud sandbox, which a provider login's
value never enters; a scoped secret reaches only the sandbox it names.
257. **The task pipeline without a mount: same phases, bundles in and out (2026-10-23, the user's answer A for gates;
builds on [159], [160], [161]):** a cloud task runs the same phases as a local one. The repo goes in as a bundle by
`sbx --cloud cp`, the clone lives under `$HOME` in the sandbox, and the result comes out as a bundle that the host
verifies before fetching it into `repo.git` (S14 Q4). Host-tier gates are unchanged. There is one gate list for both
locations: this repo's `/tmp/target` becomes `$HOME/target`, the docs say to build under `$HOME` in the cloud, and
`doctor` warns when a cloud repo's gates name `/tmp`. Grounds: S14 Q5 found `/tmp` is a tmpfs of half the memory and a
cargo target folder filled it; one list keeps a repo's gates from drifting between locations.
258. **No parallel orchestration in milestone 4; an experiment after the merge (2026-10-23, the user's answer;
builds on [245](g)):** milestone 4 adds no code for running cloud tasks in parallel. After the merge, an experiment
runs 2, then 3, cloud tasks at once as separate background `task run`s, and measures tasks per hour, cost and
failures. A `--parallel` option is built only if the experiment justifies it.
259. **Every cloud sandbox has a lifetime, and sbxm shows what is running (2026-10-23, the user's answer):** every
cloud sandbox is created with `--ttl` set to its phase's time limit plus 30 minutes and `--on-timeout delete`.
Removal is retried up to 3 times (S14 Q5: the first `rm` can fail with "temporarily unavailable"). `task.json`
records each sandbox's lifetime, so a retro can total the sandbox hours. The user reads the bill; `status` and
`doctor` list the running cloud sandboxes. A daily budget is a known gap; its trigger is a surprising bill.
260. **Cloud tests: fakes in CI, one real cloud test by hand (2026-10-23, the user's answer; builds on [248]):** CI
uses `FakeBackend`; the cloud command lines and parsers are tested on S14's captured output. An ignored
`tests/real_sbx_cloud.rs`, run by hand with an environment variable, walks one tiny task through the cloud on the
small shape: egress accepting all three refusal shapes (S14 Q1), a fake-secret leak check, the TTL set, the bundle
in and out, and removal. It runs at the milestone's end-to-end check. No cloud credentials go into GitHub CI.
261. **The cloud unavailable: refuse first, stop mid-task, never fall back (2026-10-23, the user's answer; builds on
[193], [200](c)):** when the cloud can't be used, a cloud command refuses before any write, naming the cause and its
fix (`sbx login`, or the exact `sbx secret set ... --cloud` command) and saying that `--local` runs the task here.
sbxm never falls back to a local sandbox quietly. A cloud failure in the middle of a task stops it the way `sbx` not
answering does today, and `task resume` continues it in the cloud.
262. **Milestone 4's plan-level choices, as the plan recommended (2026-10-24, the user's answers to milestone 4's
P-choices):** the user confirmed each of these as `sdlc/milestone-4.md` recommends it.
(a) P1: `location` takes `"local"` or `"cloud"`, default `"local"`, and any other value is an error naming the file.
(b) P2: a short capture run by hand before slice 1 saves the cloud's real output as fixtures and tries the points S14
left open.
(c) P3: `SbxBackend` gains a location, and a cloud task gets the instance that prefixes every call with `--cloud`.
(d) P4: a cloud task's clone lives at `$HOME/work/<task id>`, the path read once with `exec printenv HOME` and recorded
in `task.json`.
(e) P6: a removal that fails with a message naming a retry is retried after 5 s, 10 s and 20 s, through an injected
sleep; any other error is reported at once.
(f) P8: a cloud task whose profile names a secret other than a provider OAuth login is refused, naming the secret and
`--local`, and a profile naming the other provider's login is checked at global scope like the harness's own ((a)
and (d)).
(g) P10: a non-strict `[template]` profile in the cloud is built from its kits, with one line saying cloud templates
aren't used.
(h) P11: the free-disk check is skipped for a cloud create, `--ignore-disk` still accepted, which narrows [194]'s
scope for a cloud create.
(i) P12: a cloud command needs `sbx` 0.46.0 or later, a constant like [213]'s, checked only for a cloud command.
(j) P13: `doctor` checks the current directory's repo when its `sbxm-task.toml` says `location = "cloud"`, and warns for
a gate command containing `/tmp/` or ending in `/tmp` as a word ((a) with (i)).
(k) P14: only `task start`, `task run` and `task review --pr N` take `--cloud`/`--local`; every other command uses
the location recorded in `task.json`.
(l) P15: `task start --workers N` is unchanged in the cloud.
(m) P16: the user edits their own `sbxm-task.toml` to build under `$HOME`, and the docs show the line; no
`sbxm-task.toml` is added to the repo.
(n) P17: when the cloud can't be read, `task status` reads only `task.json` and shows the recorded location,
`status` prints one line in place of its cloud rows and exits 0, and `doctor` shows a warning, not a failed check.
263. **A cloud worker sandbox lives through the task's automatic run (2026-10-24, the user's answers to milestone 4's
P-choices; P5, a new option (d); builds on [118], [259]):** the cloud worker sandbox lives through the task's
automatic run (worker, sandbox gates, review rounds, fix rounds) and is removed when the task stops (ready, or
stopped for any reason). `task resume` rebuilds it from the recorded profile and a bundle of the task's branch when
needed, as it rebuilds a gone worker sandbox today [118]. Its TTL is the worker's time limit + every sandbox gate's
timeout + `fix_rounds` x (the reviewer's time limit + the worker's time limit + every sandbox gate's timeout) + 30
minutes. Reviewer sandboxes stay one per round, each with a TTL of the reviewer's time limit + 30 minutes. This
gives [259]'s "its phase's time limit" its meaning for the worker sandbox. Grounds: fix rounds keep a warm build (a
cold build is 233 s on the large shape, S14 Q5), and nothing is billed while a task waits for the user (up to 2h25m
from open to merge in the loop trial).
264. **A strict cloud sandbox denies every host outside a fixed provider list, and says which (2026-10-24, the user's
answers to milestone 4's P-choices; P9 (a) with (i), to match [221], [232]; builds on [255]):** the runtime list is a
fixed provider host list per harness in code plus the profile's `[template].allow`; every other host gets a
per-sandbox `policy deny`. This applies to strict worker, reviewer and fix-round sandboxes alike. The user added:
sbxm is *removing* hosts, so when something fails unexpectedly in a strict cloud sandbox, the first check is whether
a denied host is actually needed. So each sandbox's denied hosts are written to the task's `run.log`, the docs slice
adds a troubleshooting line for this check, and the plan's Risks section names it.
265. **Cloud sandbox lifetimes live in `task.json` and `run.log`; removal is unchanged (2026-10-24, the user's answers
to milestone 4's P-choices; P7 (a) kept, P18 and P19 dropped; builds on [195], [259]):** `task.json` keeps an
append-only `sandboxes` list (name, role, location, created_at, ttl_s, removed_at), and `run.log` gets one line at
each cloud create and each removal. They survive as every other retro number does today: `task rm --merged`
archives `task.json` and `run.log` [195]; a plain `task rm` loses them, as it already loses rounds and times. No
change to `task rm`, `--restart` or the archive layout, so P18 (archiving on every removal) and P19 (an archive per
task attempt) are dropped. Grounds: merged tasks, almost all of them, are archived already; changing removal for a
number nobody has needed yet goes against optimizing just in time. Gap G62 in `sdlc/evals-workflow-notes.md` names
the trigger for revisiting it.

## Round 13: auto-merge (2026-10-24)

266. **Low-risk PRs merge without the user, as an experiment (2026-10-24, the user's four answers; narrows [86] and
[187], where the user merges):** (a) scope: a PR whose risk is low, both the reviewer's level and the path-rule floor
[176], [179]; medium and high stay the user's to merge, for now. (b) It starts now, as a process: the session checks
that the latest sbxm review covered exactly the PR's current head commit, found 0 must-fix, and rated it low, then
runs `gh pr merge <N> --auto --merge --match-head-commit <sha>`, where `<sha>` is the full SHA of the commit that
review covered (the end of its `Scope:` range); GitHub refuses the request if the PR's head is not that commit, and
otherwise merges once the required checks pass. A session never pushes to a PR whose auto-merge is armed: it first
runs `gh pr merge <N> --disable-auto`, then pushes, and the new head gets a review (`task review --pr N`) before
auto-merge is armed again with the new reviewed SHA. Why: GitHub keeps auto-merge armed when someone with write access
pushes, so the SHA guard alone only covers the moment it is armed. The `sbxm/review` status check that would make
GitHub enforce this is issue #317, built if the experiment works. (c) Measure: over the next 10
auto-merged PRs, the breaks found after a merge (CI red on `main`, a revert, a later must-fix traced to one of them)
and the time from the PR being opened to its merge, GitHub's `createdAt` to `mergedAt` (the loop trial's median
for the same interval: 29m12s, `sdlc/retros/2026-10-23-retro.md`); the user's first answer said "ready to merged"
and the user changed it to this interval on 2026-10-24 (PR 318 review S-2), because the baseline was measured from
the PR being opened. The first break pauses auto-merge and the
session tells the user, who decides. Every auto-merged PR is logged in `sdlc/batches/2026-10-24-auto-merge.md`.
(d) GitHub settings, made by the user on 2026-10-24: the ruleset `sbxm-ruleset` requires 0 approvals and no
code-owner review, requires the checks `build (ubuntu-latest)` and `build (windows-latest)` (CodeQL stays enforced by
its code-scanning rule), with 'require branches to be up to date' off (updating a branch adds an unreviewed merge
commit; a clash between two merged PRs shows as red CI on `main`, a break); auto-merge and automatic deletion of head
branches are on. Why: the user wants merges to happen without them; low risk only keeps the trust-boundary code with
them.

## Round 14: milestone 4's capture run (2026-10-24)

267. **Two of P2's outputs move to slice 9's real test (2026-10-24, the user's choice; narrows [262](b)):** the
capture run (`sdlc/spikes/m4-p2-capture.md`) saved every output and tried every point of P2 except two outputs it
couldn't produce: the not-logged-in message (it needs `sbx logout`, which signs the user out of both cloud OAuth
logins) and `rm`'s retryable "temporarily unavailable" error (it can't be triggered on demand). Until slice 9's
real-cloud test captures them, slice 3's removal retry is tested on the wording S14 recorded ("temporarily
unavailable; retry the delete"), slice 2's not-logged-in refusal on a fixture marked as made up (no run has seen the
real message), and slice 9 replaces both with real output where it gets it. With that, P2 is done. Why:
neither output can be had now without signing the user out or waiting on chance, and a wrong wording shows up at
slice 9, where it is cheap to fix.

## Round 15: PR 322 review (2026-10-27)

268. **A cloud create sbxm can't prove it made is left for the user (2026-10-27, the user's choice; refines [265]):**
a cloud sandbox is a task's own only when its `create` returned success and its lifetime entry was saved. A create
that was started but never recorded as returned (sbxm interrupted in between) is ambiguous: names are deterministic,
and no point-in-time check can tell this task's sandbox from a same-named one another caller made (PR 322 review,
three rounds). Every recovery and removal path leaves an ambiguous sandbox and its scoped secrets alone, refuses to
delete the task's record, and names the manual `sbx --cloud` commands; `--forget-ambiguous` on `task rm`, `task rm
--merged` and `task resume` drops the ambiguous entries without touching the cloud. The TTL deletes a forgotten one
eventually. Rejected: treating every `sbxm-task-*` name as sbxm's (it could delete another caller's sandbox) and a
random suffix per create (robust but changes sandbox naming across the milestone, size L). **Trigger to revisit:**
an ambiguous create happens in real use more than once.
269. **A scheduled check starts agents that prepare recurring work, as an experiment (2026-10-28, the user's six
answers; the user asked for a way to start recurring software-factory work, such as getting issues ready for a worker
and retros; builds on [83], [245](c) and [196]):** (a) the scheduled agent only prepares: it reads GitHub and the
repo and writes drafts to local files, with no PRs, labels, comments or task work; the user works through each draft
in a session, where labels are set and decisions written (the rules of [245](b) and [244] stand); starting `task run`
on labeled issues follows once `task loop` (issue 320) exists; (b) every part must be able to move to a remote
machine or the cloud later: the scheduler is only a trigger (Windows Task Scheduler now, cron or a cloud schedule
later), and the logic is a PowerShell 7 script with no Windows-only calls, tested with Pester like
`scripts/deploy-profiles.ps1` [136]; it moves into sbxm, next to `task loop`, if the experiment is adopted; (c) the
agent runs in an `sbx` sandbox, never on the host: the script gathers its inputs on the host (issue bodies, the PRs
since the last retro) and passes them in, and the sandbox reaches only the model provider, never GitHub, because the
repo is public and issue text is untrusted; (d) for the experiment the script runs `task start --spec` only, with a
spec holding the inputs and instructions, run from a dedicated checkout whose `sbxm-task.toml` has no gates; the
worker commits `draft.md`, which the script reads from the task's host-owned `repo.git` with hardened `git show`
before `task rm --spec --yes --force` (the draft is read out, never fetched as a branch, so [178]'s guard needs
`--force`); a
one-shot `sbxm prompt --seed <dir> --out <file>` command is filed as an issue (size M) with the trigger "experiment
adopted"; (e) two jobs: readiness prep (an open issue with no `ready-for-worker` label and no draft yet: its open
points with a recommended answer each, its boundary cases [244] and a size check) and retro due (10 or more merged
PRs since the newest record in `sdlc/retros/`: a draft following `sdlc-retro`); a waiting-on-you digest and a cleanup
listing are later candidates; (f) daily at 07:00, run at the next chance if missed; skipped while any sbxm task or
sandbox is running, stopped early under `min_free_disk` [194]; at most one agent run per job per day and 5 issues per
readiness run; a summary file next to the drafts, plus a Windows toast in the trigger layer only; (g) measured per
draft: cost, time, the share of recommended answers the user accepts unchanged, and the time from a draft to the
issue's label; the trial is 2 weeks or 10 drafts, whichever comes first, then an assessment decides about adopting it.

270. **Antigravity sandboxes use the user's Google login through a proxy placeholder (2026-10-29, the user's choice
after spike S16; the user asked for it because Codex limits are being hit; builds on [7], [137]):** route A of #288
is adopted. A sandbox holds only a token file with placeholder values and a far-future `expiry`; a custom secret
scoped to the sandbox, on `daily-cloudcode-pa.googleapis.com` and `www.googleapis.com`, has `sbx`'s proxy swap in an
access token that a host helper refreshes with `agy`'s own OAuth client, so the refresh token never enters a sandbox.
The helper is called for every proxied request, so it caches the token and must start fast; the MCP gateway entry
(#287) is emptied before every run; a `real_sbx` check guards against `agy`'s self-updates. The API-key route [137]
stays as the fallback. The build is issue #330 (size L, five open points to answer before a worker starts).
Write-up: `sdlc/spikes/S16.md`.

## Round 16: the third retro (2026-10-30)

Number 271 is taken by issue 330's HTTP transport (`ureq`), recorded on the parked branch of PR #339.

272. **A fix round doesn't choose; it hands the question back (2026-10-30, the third retro's proposal 1,
`sdlc/retros/2026-10-30-retro.md`; extends [188]):** when fixing a finding needs a fact about an external tool's
behavior that can't be checked from inside the sandbox, or a choice between behaviors that the issue, the decisions
and the review don't settle, the fix round leaves that finding unfixed and writes it under "Needs a decision" in
`result.md`, with the options. The rule is a bullet in `prompts/fix.md` and `prompts/fix-spec.md`. An experiment for
the next three tasks with a fix round; its measure is repeat findings per task (the third retro's window: 3 in 2 of
10 tasks, #314 and #330).

273. **External facts are checked first, and the real check runs early (2026-10-30, the third retro's proposal 2;
extends [244] and the no-open-questions rule):** before a worker starts on an issue that depends on an external
tool's behavior a spike recorded, the session re-runs the spike's key check by hand; after the worker, before the
review rounds, it runs the issue's real test (an ignored test or a by-hand smoke) and gives the result to the review.
An experiment for the next three issues that depend on an external tool; its measure is the problems found by real use
after the review rounds ended (the third retro's window: 2 on #330, and G67-G70 in milestone 4's end-to-end check).

274. **The review loop stops at the first round with 0 must-fix (2026-10-30, the third retro's proposal 3; narrows
[251]'s re-review):** that round's should-fix items are fixed by hand and re-reviewed narrowly, on the fix's diff
only; anything the narrow re-review finds outside that diff is filed as an issue with a trigger, not fixed in another
round. An experiment for the next three PRs that reach a 0-must-fix round with should-fix items; its measure is the
review rounds after the first 0-must-fix round (the third retro's window: 5, #322 three and #329 two).

275. **The gemini harness is removed (2026-10-30, the user's choice; settles [103]'s pending follow-up):** `--harness
gemini` goes from sbxm. The built-in `gemini` sbx agent can't reach Google from a sandbox here ([103], spike S5),
Gemini CLI is deprecated upstream in favor of Antigravity, and `antigravity` is already sbxm's Google harness, so the
variant only costs upkeep. The removal is issue #345 (size M; two open points about saved `gemini` sandboxes and
configs naming it, to answer before a worker starts).

276. **`sbxm-dev`'s apt step waits for the image's own `apt-get` (2026-10-31, the user's choice; issue #337, gap
G67):** the cloud failure at create wasn't the allow list. A probe kit reached `archive.ubuntu.com` (HTTP 200) during
its install, so the list is in force then. The cloud image runs its own `apt-get` at startup; the install step's
`apt-get update` failed at once on the package lists' lock (`DPkg::Lock::Timeout` covers only dpkg's lock), `|| true`
hid it, and `install` found no packages (exit 100). The fix is in the profile, not in sbxm: before `update`, the step
polls every 2 s while `pgrep -x apt-get` finds one, for at most 5 minutes, then goes on either way. Proven on the
cloud on 2026-10-31: it waited 6 s, then installed. The small race (another `apt-get` starting between the check and
`update`) is left until it is seen.

## Open research spikes
- **S1–S4:** Done 2026-09-24; see "Spike results" below.
- **S6:** Done 2026-09-24 (partial); spec `sdlc/spikes/S6.md`, evidence `sdlc/spikes/S6-results.md`, conclusions in decisions 48 and 49.
- **S6b:** Done 2026-09-25 (partial); spec `sdlc/spikes/S6b.md`, evidence `sdlc/spikes/S6b-results.md`, conclusions in decisions 59–61. Q1 (non-empty skills store) blocked and left open [60]. Pi moved to a later spike before slice 20 (needs `kit.allowedSources` [48]).
- **S7 (Pi):** Done 2026-09-27 (all questions answered); spec `sdlc/spikes/S7.md`, evidence `sdlc/spikes/S7-results.md`, conclusions in decisions 73–75.
- **S9 (M2, before slice 14):** how the Jev token reaches the Jev call — custom `sbx secret` service and host versus a host env-var fallback [36][120]; spec in `sdlc/milestone-2.md`. **Run 2026-10-02** with a fake token (PR #65, `sdlc/spikes/S9.md`): the token reaches the call through `sbx secret set-custom` scoped to an evaluator sandbox, and the proxy swaps a placeholder only in headers sent to the named host; the proposed design is not yet a decision. A spike with the real token (S12) comes before slice 14. Deferred with slice 14 [138].
- **S10 (M2b, git bundle from a sandbox):** Done 2026-10-01 (passed), recorded in decision 161.
- **S8 (GitHub access from a sandbox):** out of M2b [141]; written 2026-09-27, not run; spec `sdlc/spikes/S8.md`. Can issue workers [80–82] push, open PRs and edit issues through `sbx`'s proxy with a repo-scoped token, and do branch protection, no Workflows permission and a narrow allowlist hold? Q1–Q2 need no token; Q3–Q4 need a scratch repo and a stored `github` secret, which the repo owner doesn't plan to set up (a fork's owner may).
- **S5 (M2):** Headless runs per harness inside `sbx`. Done: Claude, Codex, Antigravity (see "Spike results" below) [93][103]. Gemini CLI is blocked/deprecated upstream and dropped as an M2a harness [103]. Pi deferred. Prompt in, run to completion, extract answer + transcript + token usage (decision 13). Leads: the v3 kit spec has `agent-sessions@1` (`prompt: ["-p", "{{.Prompt}}"]` for claude), but `sbx` 0.43 has no CLI verb for it. Route used: `sbx exec <sandbox> <harness-cli> <headless flags>` (Claude, Codex) or `sbx run --kit <pinned> <agent> <workspace>` + `sbx exec` (Antigravity).
- **S15 (templates milestone, Pi):** not run yet [204]; spec `sdlc/spikes/S15.md` (approved by the user 2026-10-15), results template `sdlc/spikes/S15-results.md`. Does a template saved from a `pi` sandbox work with `sbx create -t TAG --pull never` plus sbxm's kits, given that Pi's `agent_arg` is a kit reference of its own [73]: does the sandbox start, does Pi answer headless, do the always-loaded `AGENTS.md` and the provider secret still work, and does the kit's allow list apply? Decides whether Pi joins the milestone.

## Spike results (2026-09-24, sbx v0.43.0, Windows)

**S1: CLI surface**
- Lifecycle: `create`, `run` (creates if missing, then attaches), `exec` (**starts a stopped sandbox**), `stop`, `rm -f` (non-interactive; also deletes sandbox-scoped secrets and kit policies), `ls --json` (name, id, agent, status, last_used_at, workspaces).
- `ls --json` does **not** show kits or env, so sbxm must keep its own host-side state (sandbox → project, profile, config hash).
- Sandbox names: letters, digits, `-`, `.`; `default` is reserved. Our `[a-z0-9-]` rule is a subset, and we must also reject `default`.
- Defaults are all host CPUs and 16 GiB memory. Comparisons must pass `--cpus`/`-m` explicitly.
- Built-in agents: claude, codex, copilot, cursor, devin, docker-agent, droid, gemini, kiro, opencode, shell. **Pi is a kit**: `docker/sbx-kits-contrib/pi` (image `docker.io/sbx/pi-image`, Anthropic only: API key or OAuth).
- Workspace is mounted at a path mirroring the host (`E:\x\ws` → `/e/x/ws`).
- **Windows gotcha:** a workspace under `%TEMP%` (AppData) fails with `ERROR: failed to run sandbox container`; the same create on `E:\` works. **Corrected 2026-09-25 [56]:** any workspace on `C:` fails on this machine, not only AppData. `doctor` should check the base dir location. Disk: each harness image is several hundred MB (14 GiB free at test time).
- Experimental commands: `sbx env` (declarative `sbxenv.yaml` with plan/approve, kits, secrets, host lifecycle commands, `-y` to auto-approve) and `sbx kit`.

**S2: Kits (decision 26: yes, kits can carry the common config)**
- `sbx` 0.43 reads local directory kits in **schemaVersion "2"** format. v3 (`capabilities:`) needs a BuildKit build step and is rejected as a plain directory. Spec: `docker/sbx-kits-contrib/spec/SPEC-v2.md`.
- A v2 `kind: mixin` can carry `permissions.network`, `environment.variables`, `credentials`, `setup.install`/`setup.startup` commands, `agentInstructions`, `volumes`, `ports`, and a `files/home/` + `files/workspace/` tree. Verified live: env vars, install hook and network allow all applied.
- `requires.agent: <harness>` pins a mixin to one harness, and composing it on another is an error. This is how per-harness hooks mixins work (e.g. the ggshield kit wires a Claude Code hook).
- Composition: `sbx create --kit A --kit B <agent> <path>`, merged in flag order; remote kit refs must be pinned by commit SHA or digest.
- **Caveat:** mixin `agentInstructions` are surfaced *progressively*. The harness profile (e.g. CLAUDE.md, written as a container-only sibling of the workspace) gets an index telling the agent to read kit files "only when a kit becomes relevant… do not read them all upfront." That's not strong enough for mandatory rules.

**S3: Egress (decision 29: native)**
- Global `default-deny-all` policy. Blocked requests get HTTP 403. A mixin's `permissions.network.allow` becomes a policy scoped to that sandbox (`source: kit`). Verified: allowed host 200, other hosts 403, same host without the mixin 403.
- Built-in harness kits already allow their own provider domains, so sbxm doesn't need to add them.
- `sbx policy log [--json]` records every allowed and blocked request per sandbox, which gives an egress audit trail per contestant.

**S5 (Claude done 2026-09-24; Codex and Antigravity done 2026-09-29 [93][103]; Gemini CLI blocked; Pi deferred): headless via `sbx exec` works**
- Claude: tested `sbx exec -w <workspace-in-container> <sandbox> claude -p "<prompt>" --model <id> --max-turns N --output-format stream-json --verbose`. Exit 0 in about 6 s; the file written by the agent appeared on the host workspace.
- `exec` sessions *do* get the proxy environment (`HTTPS_PROXY`), and the claude kit already sets `apiKeyHelper` (proxy-managed key) and `bypassPermissions`. Init event reports `apiKeySource=apiKeyHelper`, `permissionMode=bypassPermissions`.
- The stream-json output has everything we capture: `system/init` (model, cwd, session_id), `assistant` events with tool calls (the transcript), and a final `result` event with `result` (final answer), `num_turns`, `duration_ms`, `total_cost_usd`, `usage` and per-model `modelUsage`.
- Cap flags: `--max-turns` (works, not listed in `--help`) and `--max-budget-usd` (real dollar cap).
- **Killing `sbx exec` on the host does NOT stop the process inside the sandbox.** Timeouts must be enforced inside the sandbox: `sbx exec <sandbox> timeout --kill-after=<grace> <limit> <harness cmd>`. Verified: exit code 124, process gone. Fall back to `sbx stop` if needed.
- **Codex** (codex-cli, `docker/sandbox-templates:codex-docker`): `codex exec [-m <model>] --json --skip-git-repo-check "<prompt>"` (`--skip-git-repo-check` is required for a workspace that isn't a git repo, which an unseeded M2 contestant's empty directory won't be — decision 5 — so the headless-run primitive must always pass it, or always `git init` first). `sbx exec` attaches a stdin that `codex exec` then blocks reading even when a prompt argument is given ("Reading additional input from stdin…"); piping empty stdin (`"" | sbx exec …`) unblocks it, so the primitive must close/redirect stdin explicitly. `--json` prints NDJSON events: `thread.started` (`thread_id`), `turn.started`, `item.completed` (`item.type: "agent_message"`, `text`: the final answer — tool calls presumably appear as other item types, unverified), `turn.completed` (`usage`: `input_tokens`, `cached_input_tokens`, `cache_write_input_tokens`, `output_tokens`, `reasoning_output_tokens` — no dollar-cost field, unlike Claude's `total_cost_usd`). `-o/--output-last-message <FILE>` gives a clean final-answer extraction alternative to parsing the last `item.completed`. No built-in budget-cap flag found in `--help` (only `-m`, `-c` config overrides, `-s/--sandbox`, `--enable/--disable` features) — Codex likely falls into decision 98's "no budget flag, timeout-only" branch. Timeout verified: `sbx exec <sandbox> timeout --kill-after=5 3 codex exec --json --skip-git-repo-check "<long task>"` → exit 124.
- **Gemini CLI is blocked, not just untested** [103]: `gemini -p "<prompt>" --output-format stream-json --yolo --skip-trust` (flags exist and look right — `-o/--output-format` takes `text|json|stream-json`, same shape as Claude's) but the built-in `gemini` sbx agent's kit grants **no Google network permissions at all**: `sbx policy log` showed `generativelanguage.googleapis.com:443` and `play.googleapis.com:443` denied by default-deny, and even interactive OAuth sign-in failed the same way (`oauth2.googleapis.com:443` denied) — there is no working route to a live Gemini call on this setup, with or without a stored `google` secret (none was stored; the sign-in failure means one couldn't be obtained either). This contradicts decision 29's assumption ("built-in harness kits already allow their own provider domains") for Gemini specifically; Claude's and Codex's kits do pre-allow their provider hosts (re-confirmed: Codex's PONG test above actually reached OpenAI). **Consequence [103]:** Gemini CLI is dropped as an M2a contestant harness in favor of Antigravity; M1's `--harness gemini` remains structurally intact (sandbox creation, home files, skills-store warning all still work) but has never made a real Gemini API call, on this machine — worth a GitHub issue (must-fix or question) once a session with GitHub access can file one, since it affects M1 users too, not just M2.
- **Antigravity** (`agy`, kit `docker.io/sbx/antigravity-kit:latest`, agent name `antigravity` per `sbx run`) [103]: not a built-in `sbx` agent — needs `sbx run --kit docker.io/sbx/antigravity-kit:latest antigravity <workspace>` (pinned-tag pattern like Pi, decisions 73–75; `latest` used here, pinning to be decided in the milestone plan). Its kit *does* pre-allow Google hosts via `forward-bypass` policy rules (not the plain proxy path): `oauth2.googleapis.com`, `www.googleapis.com`, `play.googleapis.com`, `lh3.googleusercontent.com`, `daily-cloudcode-pa.googleapis.com`, `antigravity-cli-auto-updater-*.run.app`, `antigravity-unleash.goog` — proof a correctly-built kit can pre-allow these, unlike the built-in `gemini` agent. Headless invocation: `agy -p "<prompt>" --output-format stream-json --dangerously-skip-permissions` (also has `--effort low|medium|high|max`, `--model`, its own `--print-timeout` as an alternative to `sbx exec timeout`, and `--input-format stream-json` for multi-turn NDJSON input). Verified live: NDJSON events `init` (`conversation_id`, `tools` list, `permission_mode`), `step_update` (per turn, `state: DONE`, `agent_response` steps carry `text_delta` and per-step `usage`), and a final `result` event with `status: "SUCCESS"`, `response` (final answer), `duration_seconds`, `num_turns`, and `usage` (`input_tokens`, `output_tokens`, `thinking_tokens`, `cache_read_tokens`, `total_tokens` — no dollar cost, same gap as Codex). Workspace must be off `C:` (decision 56 applies here too — first attempt on `C:\Users\james\antigravity` failed with the same "workspace directory was not mounted" error). Timeout/kill behavior not yet verified (its own `--print-timeout` may make `sbx exec timeout` unnecessary — for the milestone plan to decide which to rely on).
- Still to test: Pi headless mode and output format (deferred, decision 93); Antigravity's timeout/kill behavior; parallel sandboxes running concurrently.

**S4: Secrets (decision 30: mostly handled by sbx)**
- Values never enter the sandbox. Service secrets (anthropic, openai, google, openrouter, github, …) and custom secrets use placeholders that the proxy swaps on requests to allowed hosts. Verified: a filesystem-wide search inside the sandbox found no secret value.
- There is no command to read values back. `sbx secret ls --json` returns scope, service/env and a **masked** value, which is enough for an existence check. Secret scanning of outputs isn't needed; only placeholders can leak.
- Secrets apply at sandbox creation (existing sandboxes don't pick up new ones). Sources can be `--ref op://…` (1Password), AWS Secrets Manager or `--command`. OpenAI OAuth is supported: `sbx secret set openai --oauth`.

**S17 (2026-10-23, `sbx` v0.46.0, cloud): network rules can be removed from a running cloud sandbox**
- `policy rm network --sandbox <name> --resource <host> -f` and `policy deny network --sandbox <name> <host>` exit 0,
  print the new list and take effect at once, also for a host of the agent's built-in list. Answer 4 B holds
  [255]. Write-up: `sdlc/spikes/S17.md`.

**S16 (2026-10-29, `sbx` v0.46.0, `agy` 1.4.10): Antigravity runs on a placeholder token with a host-refreshed secret**
- A placeholder token file plus a scoped `set-custom --command` secret on two hosts gives a working headless `pong`;
  `agy` never refreshes or validates the token itself; `sbx` calls the helper per request. Adopted as [270].
  Write-up: `sdlc/spikes/S16.md`.

### Jev facts relevant to design (from docs, Jev 1.13)
- HTTP API only (`POST https://api.typesafe.ai/v1/systemone`, Bearer auth). SDKs exist only for Python and JS, so Rust calls it with `reqwest`.
- Primitives: **Noul** (P(yes)), **Choice** (one of ≤255 options + distribution), **Score** (2–10 ordered levels, probability-weighted). Returns typed answers + probabilities/confidence, not text.
- Limits: 64k tokens per request; **32k for `state` + longest question**. Text only. $0.042 per 1M input tokens; output tokens are free.
- Fits evals via composite scoring: one question per rubric dimension, with weights applied in code.
