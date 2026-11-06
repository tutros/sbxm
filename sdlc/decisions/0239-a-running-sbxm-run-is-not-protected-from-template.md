239. **A running `sbxm run` is not protected from `template rm` (2026-10-22, PR 269 review round 1 Q-1, the user
chose option A; like [233]):** template users are still only `state.json` and every `task.json` [205]. A comparison
run records nothing they read, so `template rm`, or a rebuild retiring the previous tag, can remove the tag a run
chose between two of its repeat waves; the next wave's sandboxes then fail to create and are reported as not run.
Protecting runs would mean treating every `run.json` without `completed_at` as a user, which needs rules for
interrupted runs; it waits for gap G55's trigger.
