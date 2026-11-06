253. **A repo chooses the cloud, a flag overrides one command, and the task remembers (2026-10-23, the user's answer;
builds on [248]):** the per-repo default is `[sandbox] location = "cloud"` in `sbxm-task.toml`; `--cloud` and
`--local` override it for one command. The profile stays free of any location, so one profile serves both. A task
records its location in `task.json`, so `task resume`, `task finish` and `task rm` find its sandbox where it was
created, whatever the config says later.
