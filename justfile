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

# Create one project with a sandbox for every harness, stopping each right after it is created (one running sandbox at a time, decision 304), then list them.
demo project="just-demo":
    {{sbxm}} new {{project}} --harness claude
    {{sbxm}} stop {{project}} --harness claude
    {{sbxm}} new {{project}} --harness codex
    {{sbxm}} stop {{project}} --harness codex
    {{sbxm}} new {{project}} --harness pi
    {{sbxm}} stop {{project}} --harness pi
    {{sbxm}} list

# ---- Running tasks -----------------------------------------------------------

# Long task runs use a recorded build of sbxm, not `cargo run` (docs/workflow.md): set SBXM_TASK_BIN
# to it (an .exe or a wrapper script); without it, the `sbxm` on PATH runs. PowerShell reads the
# variable itself, so a path with a quote or a space in it is never pasted into the command.

# Run `sbxm task loop` over the ready-for-worker issues, from this checkout; extra flags pass through.
task-loop *flags:
    & $(if ($env:SBXM_TASK_BIN) { $env:SBXM_TASK_BIN } else { 'sbxm' }) task loop {{flags}}

# Show every task at once, e.g. `just status --watch`.
status *flags:
    & $(if ($env:SBXM_TASK_BIN) { $env:SBXM_TASK_BIN } else { 'sbxm' }) status {{flags}}

# Print one task's run.log, e.g. `just logs --issue 574` (also `--pr N`, `--spec FILE`).
logs *flags:
    & $(if ($env:SBXM_TASK_BIN) { $env:SBXM_TASK_BIN } else { 'sbxm' }) task logs {{flags}}

# Queue a job for the loop, e.g. `just queue run --issue 594` or `just queue spec --file NAME`.
queue *args:
    & $(if ($env:SBXM_TASK_BIN) { $env:SBXM_TASK_BIN } else { 'sbxm' }) task queue {{args}}

# Register the scheduled task `sbxm task loop`, which runs the loop at logon (decision 310; scripts/loop-trigger.ps1).
loop-register:
    ./scripts/loop-trigger.ps1 -Register

# Start the registered loop now, outside any terminal.
loop-start:
    Start-ScheduledTask -TaskName 'sbxm task loop'

# Stop the loop: after its current task, or within 5 minutes while it waits.
loop-stop:
    & $(if ($env:SBXM_TASK_BIN) { $env:SBXM_TASK_BIN } else { 'sbxm' }) task loop --stop

# Rebuild both warm templates of a profile from this checkout, claude then codex, one at a time.
templates profile="sbxm-dev":
    & $(if ($env:SBXM_TASK_BIN) { $env:SBXM_TASK_BIN } else { 'sbxm' }) template build {{profile}} --harness claude --warm .
    & $(if ($env:SBXM_TASK_BIN) { $env:SBXM_TASK_BIN } else { 'sbxm' }) template build {{profile}} --harness codex --warm .

# ---- Developing sbxm --------------------------------------------------------

# Build the debug binary.
build:
    cargo build

# Run the tests (no Docker needed).
test:
    cargo test

# Pester tests for the scripts in scripts/ (needs Pester 5: `Install-Module Pester -MinimumVersion 5 -Scope CurrentUser`).
script-test:
    Import-Module Pester -MinimumVersion 5.0.0; Invoke-Pester scripts/tests -Output Minimal -CI

# Count the `git` process starts of one test file (e.g. `just count-git-starts task_merged`) by caller (fixture or sbxm) and subcommand; with `max`, fails when sbxm's own starts are over it. Needs `sh`; run by hand, never in CI (issue #444).
count-git-starts test max="":
    sh scripts/count-git-starts.sh {{test}} {{max}}

# Everything that must pass before a commit.
check:
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    cargo test
    just script-test

# Run the tests against the real sbx (needs `sbx login` and a base dir not on C:).
real-test base_dir='H:\sbxm-it':
    $env:SBXM_REAL_BASE_DIR = '{{base_dir}}'; cargo test --test real_sbx -- --ignored

# Run the tests against the real Docker Cloud Sandboxes. Costs money; needs `sbx` logged in to the cloud (see tests/real_sbx_cloud.rs).
real-test-cloud base_dir='H:\sbxm-it':
    $env:SBXM_REAL_CLOUD = '1'; $env:SBXM_REAL_BASE_DIR = '{{base_dir}}'; cargo test --test real_sbx_cloud -- --ignored

# Compare the pinned Pi and Antigravity kit tags in src/harness.rs with the newest dated tag on Docker Hub (M2a P8). Changes nothing; re-pinning is a manual step.
kit-tags:
    foreach ($kit in 'pi-kit', 'antigravity-kit') { $pinned = (Select-String -Path src/harness.rs -Pattern "docker.io/sbx/$kit`:([0-9a-z-]+)").Matches[0].Groups[1].Value; $tags = (Invoke-RestMethod "https://hub.docker.com/v2/repositories/sbx/$kit/tags?page_size=25&ordering=last_updated").results.name | Where-Object { $_ -match '^[0-9]{8}-' }; $newest = $tags | Select-Object -First 1; $note = if ($pinned -eq $newest) { 'up to date' } else { 'NEWER TAG AVAILABLE: re-pin by hand, run the real-sbx checks, log a decision' }; Write-Host "$kit pinned $pinned, newest $newest ($note)" }

# Copy profiles/* into the profiles_dir sbxm reads, asking `sbxm config profiles-dir` (see scripts/deploy-profiles.ps1).
deploy-profiles:
    ./scripts/deploy-profiles.ps1 -ProfilesDirCommand { cargo run --quiet -- config profiles-dir }

# Install `sbxm` on your PATH from this checkout.
install:
    cargo install --path .
