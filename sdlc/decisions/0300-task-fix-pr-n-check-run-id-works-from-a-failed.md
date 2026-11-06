300. **`task fix --pr N --check-run <id>` works from a failed check's log; the loop runs it once after a second
failure (2026-11-05, the user's answers on issue #448; uses [244], [292], [297], [298], [299]):** [299] leaves a PR
whose check failed twice to the user; this lets the loop try a fix first. (1) **One log-fix per PR, ever.** On a
second failure on a loop PR, before it is left to the user, the loop disarms auto-merge if armed (as [299] (3)) and
runs `task fix --pr N --check-run <id>` once, for the first new second failure of the head; the fix task's own review
and fix rounds apply inside that one run. If it doesn't end clean (an error, a stop, must-fix left, a question handed
back in its `result.md`), or the PR's new head fails twice again, the PR awaits the user as [299] (3) says; there is
no second log-fix. It is counted from the task records, not memory, so a restart can't try a PR twice: a fix task of
that PR (`fix-<N>-*`, sbxm's own folder, a record of this repo or one that can't be read) whose folder holds
`check-log.md` is its log-fix; a folder removed by hand counts as none (no `task.json` change). A log-fix refused
before it created a task isn't tried again in the same run. After a clean log-fix, auto-merge is armed again only on
[298]'s re-arm rules (the fix task's review low risk after the path-rule floor, 0 must-fix, covering the new head; the
PR armed before, or its own task and every earlier fix task pushed onto it passing the same checks) and never while
[299]'s pause stands. A log-fix that didn't end clean keeps the PR with the user through later passes (no resolve of
it in that run) and after a restart (a later resolve doesn't re-arm a PR whose log-fix record isn't finished and
clean). A low disk or a stop request leaves the log-fix for a later pass, never the disarm. One log row either way. (2)
**The log: the last 64 KiB in all, as data.** From `gh run view <id> --json jobs` (the failed jobs, in the run's
order) and `gh run view <id> --log-failed` (each line goes to the job its first field names): the last 64 KiB in
total, split evenly between the run's failed jobs (an unused share isn't passed on), cut at a line boundary (a
line longer than its job's share is dropped whole) and never inside a UTF-8 character. Each job's part starts with a line naming the run, the job and how
many lines were dropped from its start; a failed job with an empty log is that line alone. It goes to the worker (and
the in-task reviewer) as a file, `.sbxm-task/check-log.md`, kept in the task's folder too; `issue.md` names the PR,
the run and that file, and the worker's prompt (`prompts/worker-pr-fix-check.md`, the `[prompts] worker` override
key as every worker prompt) only names it: it holds output anyone's code can print, so it is never instructions in a
prompt. (3) **By hand too:** `sbxm task fix --pr N --check-run <run-id>`, the loop's path and a session's ([297]).
With `--check-run` the worker's input is the log alone, not a review's must-fix findings (no review is read). Refused
before anything is created when the run isn't listed among the runs of the PR's current head (`commit_runs`, so a
run of an older head or of another repo), passed, or is still going, each naming what to pass.
