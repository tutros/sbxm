259. **Every cloud sandbox has a lifetime, and sbxm shows what is running (2026-10-23, the user's answer):** every
cloud sandbox is created with `--ttl` set to its phase's time limit plus 30 minutes and `--on-timeout delete`.
Removal is retried up to 3 times (S14 Q5: the first `rm` can fail with "temporarily unavailable"). `task.json`
records each sandbox's lifetime, so a retro can total the sandbox hours. The user reads the bill; `status` and
`doctor` list the running cloud sandboxes. A daily budget is a known gap; its trigger is a surprising bill.
