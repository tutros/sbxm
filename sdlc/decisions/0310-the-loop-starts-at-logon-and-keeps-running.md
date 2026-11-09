310. **The loop starts at logon and keeps running (2026-11-09, the user's answers on issue #527, slice 3 of #476;
uses [245], [294], [299], [305], [309]; the pattern of `scripts/factory-trigger.ps1`, [269]):** the loop had to be
started from the user's terminal or the agent's session, and stopped on an empty queue. Now it starts by itself at
logon and waits for work. (1) **`task loop --wait`:** with an empty queue folder and no ready issue, the loop does not
stop: it says so once and checks again every 5 minutes (`PAUSE_POLL`) for a new job file or a newly labelled issue,
until a stop request. Only the scheduled start uses it; a loop without `--wait` stops as before. `--wait` with
`--issue` is refused (a list has an end). An issue the loop refused before any write stays skipped until the loop
starts again, as in one run of a loop without `--wait`. (2) **The loop checkout and the known binary.** At each start
the script `scripts/loop-trigger.ps1` refuses to start when the loop checkout (a parameter; this machine
`H:\sbxm-e2e\loop-checkout`) has local changes (`git status --porcelain` not empty), saying why and overwriting
nothing; otherwise it runs `git fetch` and `git checkout --detach origin/main` there. It runs the binary through a
wrapper path given as a parameter (this machine `H:\sbxm-e2e\sbxm.ps1`), never an automatic build. The checkout is
reset once per start of the script, not before each restart. (3) **Restarts only after an error; exit code 3 for a
stop that needs the user.** The loop exits **3** when it stops on a lapsed sign-in, a missing secret, two parked
tasks in a row or the usage waits' total (before, all 1); other errors stay 1; a stop request stays 0. The script
restarts the loop after an exit that is neither 0 nor 3, at most 3 times in an hour, 10 minutes apart; exit 0 or 3,
or a 4th failure within the hour, leaves it down until the next logon or a start by hand. The restarting is the
script's (tested with Pester), not Task Scheduler's restart setting. (4) **A task cut off when the loop died is
resumed once.** At start, holding the loop lock it took over from a loop whose process is gone, the loop finds the
issue tasks of its repo that loop was running: not finished, `interrupted` as `task status` decides it, and recorded
as worked on by that dead loop's process (how [294] finds the loop's running task; the loop's claim, `hooks.task_loop`,
is written only at a ready task's finish, so it can't name a task being worked on). It continues each with `task
resume` (its finish, for one cut off at `ready`), counting as the loop's one resume after an environment stop: a
second failure parks it as before. Found, the task is marked at once, under its lock, as owed its resume
(`hooks.task_loop_resumed` holds the cut-off process), so a loop that stops or fails before the resume (a stop
request, a queued job's error) leaves it to the next loop, which resumes a task still owed for the process its record
names even with no stale lock left (review M-1); a command run by hand that took the task since then makes it the
user's. When the loop takes the resume the mark becomes `true`; a task marked `true` and cut off again is parked,
never resumed a second time. Each gets a log row. Never `task run`
again; a task run by hand, or a finished one, is left alone. (5) **Starts at logon, only while the user is logged on**
(Docker Desktop and `sbx` run in the user's session). Before each start of the loop the script waits up to 10 minutes
for `sbx ls` to succeed, checking every 30 seconds; still failing counts as an error under (3). A loop started by
hand while the scheduled one runs is refused by the loop lock, as before. (6) **Stopping: `sbxm task loop --stop`;
removing: `-Unregister`.** `--stop` writes a stop request (`<base>/.sbxm/task-loop.stop`, next to the loop lock); the
running loop treats it as a first Ctrl-C: it stops after the current task, or within 5 minutes while waiting or
paused, removes the request and exits 0, so it is not restarted (it is back at the next logon). `--stop` with no loop
running (no lock, or one whose process is gone) says so and writes nothing; a request left from before, with no loop
running when one starts, is removed at that start and not obeyed. `scripts/loop-trigger.ps1 -Register` registers (or
replaces) the Task Scheduler job `sbxm task loop`: at the logon of the current user, interactive only,
`-MultipleInstances IgnoreNew`, no time limit; `-Unregister` removes it, and with none registered says so and exits 0.
Toasts are slice 4.
