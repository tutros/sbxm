309. **`task loop` drains a queue folder of job files (2026-11-09, the user's answers on issue #525, slice 2 of #476;
uses [245], [298], [299], [305], [195]):** a long sbxm command had to be run from the user's terminal or watched from
the agent's session. Now the agent or the user writes a job file and the loop runs it. (1) **Four kinds in this
slice:** `review` (`task review --pr N`), `fix` (`task fix --pr N`, optional `--check-run ID`), `run` (`task run
--issue N` of an issue labeled `ready-for-worker`, then what the loop does after a task's run: its finish, auto-merge,
the findings filed) and `rm-merged` (`task rm --merged --yes`). The spec task and `template build` are slice 2b
(#524). (2) **A job file is JSON with a fixed kind**, one file per job in `<base_dir>/.sbxm/queue/` named `*.json`:
`{"kind": "review", "pr": 465}`, `{"kind": "fix", "pr": 465, "check_run": 42452742075}`, `{"kind": "run", "issue":
521}`, `{"kind": "rm-merged"}`. Parsed with no unknown field allowed: an unknown kind, a missing or extra field, a
wrong type, a number 0 or bad JSON is **refused** (a result file saying why; nothing runs). No free-form flags, so
nothing like `--force` can be queued. Any other file (`x.json.tmp` while being written) is left alone. (3) **Order:**
before each new issue, after the loop's conflict resolves (298) and its checks of the runs (299), and before its pause
on the PRs awaiting the user, the loop runs **every** queued job, oldest first by the file's modified time, ties by
file name, a job written meanwhile too; then the next issue. A stop request between jobs is honored as between issues:
the jobs not taken stay queued. Jobs don't count toward `--cap`. (4) **"A PR sbxm opened"** (for `review` and `fix`):
a task record of the same repo, live under the task folders or archived under `.sbxm/task-archive/` (195), whose `pr`
is that PR. Without one the job is refused, naming the command to run by hand. A PR merged or closed when the job
starts is skipped. A `run` job is refused for every reason `task loop --check` gives (not labeled, no size, open
points, an unmerged dependency, what `task run` refuses before any write, a branch on origin). (5) **An empty folder
and no ready issues: the loop stops as today**; waiting for new work is slice 3. Both loop modes (the labeled queue
and `--issue N` lists) drain the folder; a missing folder is not created. (6) **Interrupted jobs are never re-run.**
Taking a job moves its file to `queue/running/`, and once it starts a task the running file names that task; its end
writes `queue/done/<job>.json` and removes the running file. At loop start (under the loop lock) a file in `running/`
gets a result with outcome `interrupted`, naming the task it started (if any), that task's stage and status, and the
command that continues it (issue 512's `next`), and is removed. Re-queueing is a new job file. **The result file**
(`queue/done/<job>.json`, for a refused or skipped job too) holds the job as read (`job`, or `job_text` with the raw
text when it isn't JSON), `outcome` (`done`, `failed`, `stopped`, `refused`, `skipped`, `interrupted`), `reason`,
`started_at` and `ended_at` (UTC, as `task.json` writes them), `task` (the task id), `url` (the PR the task opened or
works on) and `next` (the next step sbxm names, if any). A name already in `done/` is never overwritten: the result
goes to `<job>-2.json`, and on. **The loop's log** (305) gets one row per job, its Issue column naming the job (`job
review #465`). A `run` job that stops the loop (a lapsed sign-in, the usage limit, an error) writes its result first.
Every folder of the queue is refused before the loop's lock when it is a link, isn't really under the base dir, or
isn't a folder.
