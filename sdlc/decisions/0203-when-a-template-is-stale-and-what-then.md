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
