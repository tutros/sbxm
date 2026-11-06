214. **Templates belong to the profile, not the project (2026-10-15, PR 253 review round 2 Q-4, the user agreed
with the recommendation):** `[template]` in a project's `sandbox.toml` [57] is refused. A project override that adds
a baked input (`setup.install`, `network.allow` or `network.deny`) is refused for a strict profile, naming the
override, and gets today's installs plus a warning for a non-strict one. Why: a template is built and recorded per
profile and harness, so a project variant has no template that can match it; silently dropping the project's
build inputs was rejected, and per-project templates wait for a need.
