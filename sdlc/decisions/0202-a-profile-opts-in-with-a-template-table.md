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
