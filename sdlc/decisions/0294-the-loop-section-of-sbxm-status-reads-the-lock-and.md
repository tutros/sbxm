294. **The Loop section of `sbxm status` reads the lock and the task records (2026-11-05, the user's answers of
2026-11-02 and option (a) of 2026-11-04 on issue #376; part 3 of #320; supersedes the 2026-11-02 plan of a lock that
holds the current issue and stage):** (1) State comes from the loop's lock plus the task records, no new state file
and no new write. The lock (`<base>/.sbxm/task-loop.lock`, the loop's process identity) says whether a loop runs; a
lock whose process is gone (the pid and start-time check of `run.log`, [163]) shows as `not running`, naming the
stale lock. The running task and its stage come from the issue task's `task.json` that is still running (`status == running`) and whose recorded `process` equals the lock's process (the user's answer of 2026-11-04, as changed by the user on 2026-11-06 after review M-2): the loop runs its phases in its own process, and its marker (`hooks.task_loop`) is written only when it claims a ready task for `task finish` ([245]), so the marker can't identify the task being worked on. The marker keeps its one meaning (the loop finished this task; its PR counts toward the cap). The tasks done are the marked records finished since the lock's process
started. The loop's log is not read: `status` reads only the base folder. (2) The queue needs a GitHub call, at most
one per minute, kept in memory between `--watch` frames (a plain `status` calls once); the snapshot holds the queue
and the PRs awaiting the user, and a failure is cached for the minute too. It is made whether or not a loop runs, so the queue and the PRs awaiting you show after the loop stops too. A failed call shows `queue: unreadable (<first line>)` and the rest still
shows, as the cloud section does. (3) The PRs awaiting the user are counted by the loop's own rule
(`queue::awaits_user`, [293]). On a terminal each line is cut to the width and the queue shows 10 issues, then
`… N more`; a non-terminal is never cut. The queue is the label queue until the continuous-loop queue the user
named is looked into.
