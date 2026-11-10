311. **The loop shows a Windows toast when the user is needed (2026-11-09, the user's answers on issue #530, slice 4
of #476; uses [293], [299], [309], [310]; the toast of `scripts/factory-trigger.ps1`, [269]):** with the loop
started at logon (310) no terminal shows its lines, so the user learns it needs them from a toast; clean low-risk
work stays quiet. (1) **A toast only when the user has something to do:** (a) a PR a loop opened or continued awaits
the user's merge: a risk above low after the path-rule floor in any of its reviews (its own task's and every
resolve's or log-fix's pushed onto it, since a later narrow review doesn't clear an earlier risk, [298] (3), [300]), or
a draft (a work stop, must-fix left), read from the review that covered the PR's head; (b) a queued
job ends `failed`, `stopped`, `refused` or `interrupted` (309); (c) the loop stops on a cause that needs the user: a
lapsed sign-in, a missing secret, two parked tasks in a row, the usage waits' total (310's exit 3); (d) `main`'s second
CI failure pauses arming (299); (e) a task the loop ran (an issue or spec task, a resolve or log-fix, a queued
job's task) hands a question back: its reviewer's question findings in `review.md`, or its worker's "Needs a
decision" in `result.md`. Quiet: clean low-risk work, auto-merges, jobs ending `done` or `skipped`, a loop stopped on request, an empty queue. (2) **The loop
shows them itself, through a `Notifier` trait** (`src/notify.rs`, like `SandboxBackend` and `GitHubBackend`): the
rules of when are the loop's code only. The Windows notifier runs `powershell.exe` (Windows PowerShell 5.1, where
the WinRT types load) with the toast snippet of `factory-trigger.ps1`; the toast's XML, its text XML-escaped, goes in
through an environment variable, never the command line. On other systems it writes one line on the loop's output
instead. Tests use a fake that records the events; only `main` wires the real notifier, so tests and CI never show a
toast. (3) **Content and click:** the title names the event and its target (`PR #526 awaits your merge (medium)`,
`Job run #529 refused`, `Loop stopped: sbx sign-in lapsed`, `main is red: auto-merge paused`, `Task issue-530 has a
question for you`); the body is one line of reason and the next step sbxm already names (the job result's `next`,
the loop's stop line, `--clear-pause`). A click opens the PR or issue URL (protocol activation; the failed run's URL
for `main`), or with no URL the loop's log file. (4) **One toast per event, ever.** Keys: PR number and head commit
(a new push after a fix round may toast again), the job's file name, the loop's start time and stop cause, `main`'s
commit, the task and a hash of its question. The keys shown go into `<base_dir>/.sbxm/notified.json`, so a restart
doesn't repeat them; no reminders. An unreadable file is a warning and read as empty. (5) **On by default for every
loop**, by hand or scheduled; off per machine with `[notify] toasts = false` in `config.toml` (then nothing is shown
and `notified.json` isn't written). A toast that fails to show is one warning line, the loop goes on and the key is
still recorded.
