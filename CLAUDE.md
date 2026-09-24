# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Status

Pre-implementation. No Rust crate exists yet. Sources of truth:
- `idea.md`: original brief
- `milestone-1.md`: current implementation plan (crate layout, config schema, kit mapping, commands, build order).
- `decisions.md`: numbered design decisions and open research spikes. **Read it before designing anything**, and add new decisions there instead of silently departing from it.

## Workflow

Each SDLC phase has a project skill in `.claude/skills/`; follow it for that phase:
- `sdlc-planning`: interview rounds, numbered decisions, spikes, milestone plan.
- `sdlc-implementation`: vertical slices, TDD (red first), smallest change per step, commit after every green step (`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`).

Update this file with real `cargo` commands and module layout once the crate is scaffolded.

## What `sbxm` is

A Rust CLI (crate/binary name `sbxm`) that wraps Docker Sandboxes (`sbx`) to:

1. **Milestone 1: sandbox lifecycle.** Create or reuse a sandbox for a project at `<base_dir>/<project>`, applying a shared, versioned common config (egress allowlist, secret references, instructions file, per-harness hooks/guardrails). Commands: `new`/`open`, `list`, `stop`, `rm`, `config show`, `doctor`.
2. **Milestone 2: comparisons.** Run 2–4 *contestants* (`harness × model × optional config profile`) headless and in parallel on the same task, each in a fresh snapshot of an optional seed directory. Capture answer, diff and transcript, then evaluate with any subset of executable checks, rubric-based LLM judge, Jev (TypeSafe System One), cosine similarity (text answers only) and human review.

## Architectural constraints (from decisions.md)

- **sbxm enforces nothing itself:** `sbx` handles egress (deny-by-default proxy), secrets (proxy-injected placeholders) and isolation. sbxm compiles config into **schemaVersion "2" mixin kits** (one shared plus one per harness pinned with `requires.agent`) and passes them to `sbx create --kit`. `sbx ls --json` doesn't expose kits or env, so sbxm keeps its own host-side state. See "Spike results" in `decisions.md` for verified `sbx` behavior.
- **Mandatory instructions** go in each harness's always-loaded user-level file via the per-harness mixin's `files/home/`. Kit `agentInstructions` is only surfaced on demand, so it's for reference material only.
- **Timeouts must be enforced inside the sandbox** (`sbx exec … timeout --kill-after=… <limit> <cmd>`). Killing `sbx exec` on the host leaves the agent process running.
- **Base dir must not be under `%TEMP%`/AppData:** `sbx` fails to mount workspaces there on Windows.
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
