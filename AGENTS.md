# AGENTS.md

This file provides guidance to coding agents (Claude Code reads it through `CLAUDE.md`) when working with code in this repository.

## Status

Milestone 1 complete (slices 0–21, end-to-end check passed 2026-09-27). Milestone 2 planning started 2026-09-29 at the user's request, from inside an sbxm sandbox (`sbxm-sbxm-m2-claude`), per decisions 92–123: one plan, two sub-milestones — **M2a** (comparisons, `milestone-2.md`, full vertical-slice detail) and **M2b** (folding `scripts/issue-workers.ps1` into sbxm, rough outline in the same file, refined once M2a's shared headless-execution primitive exists). Implementation hasn't started. All the issues filed before planning are fixed and merged (2026-09-28, PRs #14–#26; none open), and `scripts/issue-workers.ps1` is frozen to bug fixes until M2b (decision 88). A live spike during planning found the built-in `gemini` sbx agent has no working Google network permissions on this setup (and Gemini CLI is deprecated upstream in favor of Antigravity) — M2a uses `antigravity` instead as the Google-model contestant harness; `--harness gemini`'s fate in sbxm is a separate, not-yet-filed GitHub issue (decision 103). `README.md` is the user-facing documentation: keep it in step with behavior changes. Sources of truth:
- `idea.md`: original brief
- `milestone-1.md`: milestone 1's implementation plan (crate layout, config schema, kit mapping, commands, build order) — done, kept for reference.
- `milestone-2.md`: milestone 2's implementation plan (M2a comparisons, M2b issue-worker migration).
- `decisions.md`: numbered design decisions and open research spikes. **Read it before designing anything**, and add new decisions there instead of silently departing from it.

## Workflow

The repo is `tutros/sbxm` on GitHub (public since 2026-09-27, `MIT OR Apache-2.0`; `origin`, branch `main`). Code review findings are GitHub issues there (decision 79); close them from fix commits with `Fixes #n`. Every change, docs and planning files included, goes to a branch and a PR after an initial code review, and the independent reviewer runs on the PR before the user merges; never push to `main` (decision 86).


Each SDLC phase has a project skill in `.claude/skills/`; follow it for that phase:
- `sdlc-planning`: interview rounds, numbered decisions, spikes, milestone plan.
- `sdlc-implementation`: vertical slices, TDD (red first), smallest change per step, commit after every green step (`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`).
- `sdlc-code-review`: review a slice before reporting it, a worktree before merging, or a milestone at its end; findings need evidence, fixes go back through TDD.

## Commands

```
cargo build
cargo run -- --help                          # run the CLI
cargo test                                   # all tests (no Docker needed)
cargo test --test cli help_prints_usage      # a single test
$env:SBXM_REAL_BASE_DIR='E:\sbxm-it'; cargo test --test real_sbx -- --ignored   # real sbx; needs `sbx login`, dir not on C: (decision 56)
cargo fmt --check
cargo clippy --all-targets -- -D warnings
just script-test                             # Pester 5 tests for scripts/issue-workers.ps1 (decision 91)
```

The `justfile` wraps these (`just check`, `just real-test`) plus representative sbxm commands (`just` lists them).

## Code layout

Crate `sbxm` (edition 2024), lib + bin. Modules grow slice by slice, following the crate layout table in `milestone-1.md`.
- `src/lib.rs` / `src/main.rs`: everything lives in the lib; `main` parses args and passes `SbxBackend` to commands.
- `src/cli.rs`: `clap` derive definitions.
- `src/config.rs`: config dir resolution (`SBXM_CONFIG_DIR` or `~/.config/sbxm`), `GlobalConfig` loading (unknown keys are errors, `default_harness` has its own message; decisions 76, 77), and `Profile` loading (unknown keys are errors; env names/values and `skills.store` are checked on load, `SBXM_` env names are reserved), `Profile::with_project` merging `.sbxm/<project>/sandbox.toml` over it (decision 57), plus instruction files (read at load, contents hashed, paths must stay inside the defining file's folder; decision 62), `setup.install` steps, `harness.claude.home_files` (folder read at load, no links, kit-owned files refused; decision 63) and `harness.claude.managed_settings` (a JSON object, stored compact; decision 64), and the config hash per harness (`config_hash`, `GlobalConfig::current_hash`; decisions 55, 69).
- `src/harness.rs`: `Harness` (`claude`, `codex`, `gemini`, `pi`; a `clap::ValueEnum` for `--harness`, default `claude`): `sbx` agent name and `agent_arg` (for Pi, the Docker Hub kit pinned to a tag; decision 73), always-loaded instructions file, `unsupported` settings (warned about) and `applied` (the profile without them, which the kit and hash use; decision 69).
- `src/project.rs`: project name validation (decisions 33, 44), sandbox name, metadata dir.
- `src/backend/`: `SandboxBackend` trait (`create`, `list`, `stop`, `remove`, `attach`, `validate_kit`, `secret_services`, `version`), `SbxBackend` (shells out to `sbx`; parsers unit-tested against captured output in `src/backend/fixtures/`), `FakeBackend` (records calls, plus an ordered `log()`; `failing_create()`, `failing_remove()`, `with_sandboxes()`, `with_invalid_kit()`, `with_secrets()`, chainable `and_sandboxes()`, `with_version()`, `without_sbx()`, `failing_list()`).
- `src/headless.rs`: the shared headless-execution primitive (M2a slice 1, decisions 94, 114): `run(backend, sandbox, workdir, harness, prompt, opts, timeout)` wraps the harness's command in `timeout -v --kill-after=10 <secs>` inside the sandbox, calls `SandboxBackend::exec` and returns a `HeadlessResult` (`Completed`/`TimedOut`/`Failed`, answer, raw transcript, usage); a timeout is recognised by `timeout -v`'s stderr line, not by exit code alone. Only Claude has an adapter so far (`Harness::headless_argv`/`parse_headless_output`); fixtures in `src/fixtures/`.
- `src/state.rs`: `.sbxm/<project>/state.json` (per harness: sandbox, workspace, created_at, profile, config_hash); `load_all` scans every project.
- `src/kit.rs`: the only module that knows the v2 kit format; builds the `common` mixin (network, env plus `SBXM_CONFIG_HASH`/`SBXM_PROFILE`, `setup.install`, reference instructions as `agentInstructions`) and the `harness-<h>` mixin (`requires.agent: <h>`, mandatory instructions as the harness's home file, e.g. `.claude/CLAUDE.md`, `.codex/AGENTS.md`, `.gemini/GEMINI.md` or `.pi/agent/AGENTS.md`; for Claude also the `home_files` and a root install step writing `/etc/claude-code/managed-settings.json`), and writes each `spec.yaml` (`serde_norway`) plus home files to `kits/<hash-prefix>/{common,harness-<h>}/`. Kit tests use `insta` inline snapshots.
- `src/seed.rs`: copying a seed dir into a new workspace (rejects links).
- `src/confirm.rs`: `Confirm` trait for destructive prompts; `Terminal` asks on the TTY. Tests use a scripted fake.
- `src/commands/`: one file per subcommand (`config_init.rs`, `config_show.rs`, `doctor.rs`, `new.rs`, `list.rs`, `stop.rs`, `rm.rs`, `open.rs`); `new`, `open`, `stop`, `rm` and `config show` take `--harness` (`rm --purge` removes every sandbox of the project; decision 70); `new` and `open` write warnings to a `warn` writer. `doctor` takes a `Host` (free space, writability, temp dirs) so tests don't depend on the machine. `open` refuses on config drift; `open --rebuild` goes through `new` with `replace`, which removes the old sandbox only after the new kit validates.
- `scripts/issue-workers.ps1` (parameters and dispatch; the functions are in `scripts/issue-workers.psm1`, tested by Pester in `scripts/tests/`, decision 91): host-side dispatcher handing GitHub issues to headless Claude agents, one sbxm sandbox per issue (decisions 80–82), then `review` runs host checks and an independent reviewer (Codex by default) in its own sandbox, with one fix round (decisions 84, 85), and `review -Pr <n>` reviews any open PR and comments on it (decision 87); `profiles/sbxm-dev/` is the profile it uses (Rust 1.93.0, crates.io egress, no GitHub access; spike S8 covers adding it).
- `tests/`: one file per command (plus `config_hash.rs`, `kit.rs`, `profile.rs`, `project_config.rs`, `secrets.rs`, `instructions.rs`, `setup_install.rs`, `home_files.rs`, `managed_settings.rs`, `config_show.rs`, `doctor.rs`, `harness_codex.rs`, `harness_gemini.rs`, `harness_pi.rs`). CLI-level tests use `assert_cmd` with `SBXM_CONFIG_DIR` pointing at a `tempfile` dir; tests that reach the backend call command functions in-process with `FakeBackend`, using the `Env` and `dir_link` helpers in `tests/common/`. `tests/real_sbx.rs` walks one sandbox through new (incl. egress 200/403, profile env, the hash env vars, the mandatory CLAUDE.md, an install step, a home file and managed settings) → rebuild → list → stop → rm (needs the `anthropic` secret stored), a Codex sandbox (`AGENTS.md` in `codex debug prompt-input`, then `stop`/`rm --harness codex`), a Gemini sandbox (`GEMINI.md` present, then `stop`/`rm --harness gemini`), a Pi sandbox (`~/.pi/agent/AGENTS.md` present, then `stop`/`rm --harness pi`), plus a missing-secret refusal. Never touch the real config.

On Windows, cargo can print `error finalizing incremental compilation session directory … Access is denied`. It's a harmless filesystem-lock warning, not a lint failure.

The Bash tool here turns every `\\` in a command into `\` before bash runs it: in heredocs, single quotes and scripts alike (a lone `\` is unaffected; the PowerShell tool keeps backslashes intact). A `PreToolUse` hook (`.claude/hooks/block-double-backslash.py`, registered in `.claude/settings.json`) blocks any Bash command containing `\\`. Write files that need backslashes (JSON fixtures, Windows paths, Rust `\n` escapes inside strings) with the Write/Edit tools, or use PowerShell. Exception: the Write/Edit tools decode a JSON-style unicode escape (backslash, `u`, four hex digits) into its character, so write those with PowerShell (`[char]92`) or build them in code (`char::from(92)`).

## What `sbxm` is

A Rust CLI (crate/binary name `sbxm`) that wraps Docker Sandboxes (`sbx`) to:

1. **Milestone 1: sandbox lifecycle.** Create or reuse a sandbox for a project at `<base_dir>/<project>`, applying a shared, versioned common config (egress allowlist, secret references, instructions file, per-harness hooks/guardrails). Commands: `new`/`open`, `list`, `stop`, `rm`, `config show`, `doctor`.
2. **Milestone 2: comparisons.** Run 2–4 *contestants* (`harness × model × optional config profile`) headless and in parallel on the same task, each in a fresh snapshot of an optional seed directory. Capture answer, diff and transcript, then evaluate with any subset of executable checks, rubric-based LLM judge, Jev (TypeSafe System One), cosine similarity (text answers only) and human review.

## Architectural constraints (from decisions.md)

- **sbxm enforces nothing itself:** `sbx` handles egress (deny-by-default proxy), secrets (proxy-injected placeholders) and isolation. sbxm compiles config into **schemaVersion "2" mixin kits** (one shared plus one per harness pinned with `requires.agent`) and passes them to `sbx create --kit`. `sbx ls --json` doesn't expose kits or env, so sbxm keeps its own host-side state. See "Spike results" in `decisions.md` for verified `sbx` behavior.
- **Mandatory instructions** go in each harness's always-loaded user-level file via the per-harness mixin's `files/home/`. Kit `agentInstructions` is only surfaced on demand, so it's for reference material only.
- **Timeouts must be enforced inside the sandbox** (`sbx exec … timeout --kill-after=… <limit> <cmd>`). Killing `sbx exec` on the host leaves the agent process running.
- **Base dir must not be on `C:`:** on this Windows machine `sbx` fails to mount any workspace on `C:` (not only `%TEMP%`/AppData; decision 56). Use `E:`.
- **`SandboxBackend` trait:** all `sbx` interaction goes through it (shelling out to the `sbx` CLI). A `bollard` backend may be added later, and tests use a fake backend by default.
- **Harness adapters:** each harness (Claude Code, Codex, Gemini CLI, Pi) gets an adapter that owns its headless command, transcript/usage parsing, native config file names (`CLAUDE.md`/`AGENTS.md`/`GEMINI.md`, hooks), and provider egress domains. If a configured feature is unsupported, warn loudly; never drop it silently.
- **Config:** TOML layered global (`~/.config/sbxm/config.toml`) → named profile → project `sandbox.toml`. Record a hash of the merged config with every sandbox and run. A changed hash on an existing sandbox requires `--rebuild`.
- **Security is a core goal:** egress is deny-by-default. Config only *names* secrets (values live in `sbx secrets`), and secret values must never reach results files. Seed directories are copied into a fresh git repo with a single baseline commit (no history or remotes). Project names are restricted to `[a-z0-9-]`, and reserved Windows names are rejected. `rm` deletes project directories only with `--purge` plus confirmation.
- **Evals:** rubric criteria are structured (`id`, `kind` = `pass_fail` | `scale` with levels, `weight`, `notes`) so Jev, the LLM judge and humans score the same criteria. Jev-specific questions are a separate extension point. Contestants are scored independently by default and ranked in code, with blind A/B/C labels for judges and humans.
- **Jev:** HTTP API only (no Rust SDK; use `reqwest`). 32k-token limit on state plus longest question, so it never receives transcripts, and large diffs are split per file. Docs: https://docs.typesafe.ai/llms.txt. The `typesafe@typesafe-ai` Claude Code plugin (skill `/typesafe:typesafe-ai`) is installed for reference.
- **Results:** plain files under `<base>/<project>/runs/<run-id>/<contestant>/` are the source of truth. `--repeat N` is part of the run model. Timed-out runs keep their partial output and are still evaluated.

## Environment Notes

- Primary platform is Windows (PowerShell) with Docker Desktop. Keep path handling cross-platform (`std::path`, no hard-coded separators).
- Sandbox names are `sbxm-<project>-<harness>`. sbxm metadata lives in `<base>/.sbxm/<project>/`, never inside the mounted workspace.
