261. **The cloud unavailable: refuse first, stop mid-task, never fall back (2026-10-23, the user's answer; builds on
[193], [200](c)):** when the cloud can't be used, a cloud command refuses before any write, naming the cause and its
fix (`sbx login`, or the exact `sbx secret set ... --cloud` command) and saying that `--local` runs the task here.
sbxm never falls back to a local sandbox quietly. A cloud failure in the middle of a task stops it the way `sbx` not
answering does today, and `task resume` continues it in the cloud.
