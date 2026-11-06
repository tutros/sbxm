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
