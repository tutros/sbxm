# Representative sbxm commands. Run `just` to list the recipes.
# Recipes run sbxm through `cargo run`, so they always use the current code.
# Run `just install` for a plain `sbxm` command on your PATH.

set shell := ["pwsh", "-NoLogo", "-NoProfile", "-Command"]
set windows-shell := ["pwsh.exe", "-NoLogo", "-NoProfile", "-Command"]

sbxm := "cargo run --quiet --"

# List the recipes.
default:
    @just --list --unsorted

# ---- Using sbxm -------------------------------------------------------------

# Write a starter config and `default` profile (refuses to overwrite).
init:
    {{sbxm}} config init

# Check sbx, the config, every profile and project, and the base dir.
doctor:
    {{sbxm}} doctor

# List sbxm sandboxes with status, config drift and orphans.
list:
    {{sbxm}} list

# Create a project and its sandbox, e.g. `just new demo codex`.
new project harness="claude" profile="":
    {{sbxm}} new {{project}} --harness {{harness}} {{ if profile == "" { "" } else { "--profile " + profile } }}

# Create a project from a seed folder, e.g. `just seed demo E:\seeds\app`.
seed project dir harness="claude":
    {{sbxm}} new {{project}} --seed "{{dir}}" --harness {{harness}}

# Attach to a project's sandbox, creating it if needed.
open project harness="claude":
    {{sbxm}} open {{project}} --harness {{harness}}

# Recreate a sandbox from the current config (session history is lost; the workspace is kept).
rebuild project harness="claude":
    {{sbxm}} open {{project}} --harness {{harness}} --rebuild

# Stop a project's sandbox.
stop project harness="claude":
    {{sbxm}} stop {{project}} --harness {{harness}}

# Remove a project's sandbox and state; the workspace is kept.
rm project harness="claude":
    {{sbxm}} rm {{project}} --harness {{harness}}

# Remove every sandbox of a project and delete its workspace (asks first).
purge project:
    {{sbxm}} rm {{project}} --purge

# Print the merged config, its hash and the kits, without creating anything.
show project="" harness="claude":
    {{sbxm}} config show {{project}} --harness {{harness}} --kits

# Create one project with a sandbox for every harness, then list them.
demo project="just-demo":
    {{sbxm}} new {{project}} --harness claude
    {{sbxm}} new {{project}} --harness codex
    {{sbxm}} new {{project}} --harness gemini
    {{sbxm}} new {{project}} --harness pi
    {{sbxm}} list

# ---- Developing sbxm --------------------------------------------------------

# Build the debug binary.
build:
    cargo build

# Run the tests (no Docker needed).
test:
    cargo test

# Pester tests for scripts/issue-workers.ps1 (needs Pester 5: `Install-Module Pester -MinimumVersion 5 -Scope CurrentUser`).
script-test:
    Import-Module Pester -MinimumVersion 5; Invoke-Pester scripts/tests -Output Minimal -CI

# Everything that must pass before a commit.
check:
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    cargo test
    just script-test

# Run the tests against the real sbx (needs `sbx login` and a base dir not on C:).
real-test base_dir='E:\sbxm-it':
    $env:SBXM_REAL_BASE_DIR = '{{base_dir}}'; cargo test --test real_sbx -- --ignored

# Install `sbxm` on your PATH from this checkout.
install:
    cargo install --path .
