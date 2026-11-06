269. **A scheduled check starts agents that prepare recurring work, as an experiment (2026-10-28, the user's six
answers; the user asked for a way to start recurring software-factory work, such as getting issues ready for a worker
and retros; builds on [83], [245](c) and [196]):** (a) the scheduled agent only prepares: it reads GitHub and the
repo and writes drafts to local files, with no PRs, labels, comments or task work; the user works through each draft
in a session, where labels are set and decisions written (the rules of [245](b) and [244] stand); starting `task run`
on labeled issues follows once `task loop` (issue 320) exists; (b) every part must be able to move to a remote
machine or the cloud later: the scheduler is only a trigger (Windows Task Scheduler now, cron or a cloud schedule
later), and the logic is a PowerShell 7 script with no Windows-only calls, tested with Pester like
`scripts/deploy-profiles.ps1` [136]; it moves into sbxm, next to `task loop`, if the experiment is adopted; (c) the
agent runs in an `sbx` sandbox, never on the host: the script gathers its inputs on the host (issue bodies, the PRs
since the last retro) and passes them in, and the sandbox reaches only the model provider, never GitHub, because the
repo is public and issue text is untrusted; (d) for the experiment the script runs `task start --spec` only, with a
spec holding the inputs and instructions, run from a dedicated checkout whose `sbxm-task.toml` has no gates; the
worker commits `draft.md`, which the script reads from the task's host-owned `repo.git` with hardened `git show`
before `task rm --spec --yes --force` (the draft is read out, never fetched as a branch, so [178]'s guard needs
`--force`); a
one-shot `sbxm prompt --seed <dir> --out <file>` command is filed as an issue (size M) with the trigger "experiment
adopted"; (e) two jobs: readiness prep (an open issue with no `ready-for-worker` label and no draft yet: its open
points with a recommended answer each, its boundary cases [244] and a size check) and retro due (10 or more merged
PRs since the newest record in `sdlc/retros/`: a draft following `sdlc-retro`); a waiting-on-you digest and a cleanup
listing are later candidates; (f) daily at 07:00, run at the next chance if missed; skipped while any sbxm task or
sandbox is running, stopped early under `min_free_disk` [194]; at most one agent run per job per day and 5 issues per
readiness run; a summary file next to the drafts, plus a Windows toast in the trigger layer only; (g) measured per
draft: cost, time, the share of recommended answers the user accepts unchanged, and the time from a draft to the
issue's label; the trial is 2 weeks or 10 drafts, whichever comes first, then an assessment decides about adopting it.
