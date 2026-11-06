282. **A stuck PR task is moved on by `task review --pr N` itself (2026-11-02, the user's answers on issue #379;
refines [169](f) and [173]'s "replaces 169(f)'s `rm` + `review --pr` loop for stranded tasks", which `resume` did for
issue and spec tasks only):** a PR task had four dead ends where only `task rm --pr N` moved it on, after which
`review --pr` started from scratch: `prepared/failed`, `gating/running` and `reviewing/running` with the process gone,
and `reviewing/completed`. (a) **No new command:** `task review --pr N` moves it on from all four, keeping 169(f)'s
one path; there is no `resume --pr`, and `resume` stays refused for a PR task (spec 5.2). `check_can_review_pr` now
asks the process probe, so gates or a review whose recorded process is still alive are still refused. (b) **Cases 1
to 3 keep the task folder:** the PR is read and its head fetched again (`repo.git` cloned again when the failed
preparation never made it), with no unchanged-head refusal (no review of it was recorded); after every check, the
reviewer sandbox and clone the interrupted run left are removed (a cloud one through its open lifetime entry), then
the gates and the reviewer run as the next round, every earlier `review-<n>.md` kept as the reviewer's context (as
for a `ready` re-review, [190]). (c) **Case 4 runs the reviewer again, no replay:** a PR task never saves
`reviewing/completed` itself (`review_pr` records `ready` in the same write), so it is handled like
`reviewing/failed`. `dead_ends(Kind::Pr)` is empty, and `tests/task_machine.rs` asserts it.
