291. **A worker never adds a crate; the user approves each one (2026-11-03, the user's answers on issue #403, gap G1;
refines [283]):** a strict sandbox can't fetch crates [200], and per-task crates.io egress would weaken strict mode
[218]. The worker prompts (`worker.md`, `worker-spec.md`, `worker-pr-fix.md`) say: when a crate is missing (a new
dependency or dev-dependency, a transitive one made direct, a version bump, a crate with a build script), don't edit
`Cargo.toml` or `Cargo.lock`, and hand it back under "Needs a decision" naming the crate, its version and why it's
needed. A branch whose commits change the lock files of a strict profile's warm template stops before its gates (and
before any reviewer or remade worker sandbox), with the same words: "this branch's own commits change them", "Needs a
decision", and putting the crate on the base in a pull request of its own; [290]'s rebuild refreshes the template once
that PR merges, and the parked task is resumed. The user approves every new crate; revisit (session approval within
limits, such as an existing indirect dependency or a dev-only crate) if these hand-backs pile up.
