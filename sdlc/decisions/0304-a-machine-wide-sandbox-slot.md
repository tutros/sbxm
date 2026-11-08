304. **A machine-wide sandbox slot: every command that creates a local sandbox first takes one (2026-11-08, the user's
answers on issue #476, answer 4, and issue #504; slice 1 of #476; uses [163], [194], [212], [253]):** this machine (16
GB of RAM) must run one sandbox at a time, and nothing stopped a `task review --pr N` started by hand while a `task
loop` ran. (1) **The setting:** `[resources] sandbox_slots = N` in `config.toml`, N >= 1; unset means no limit, and
nothing is written; `sandbox_slots = 0` is a config error naming the setting. It is not per sandbox, so it is in no
config hash. sbxm never sets it: the user sets `sandbox_slots = 1` on this machine. (2) **What holds a slot: the whole
command,** not each sandbox, from its free-space check (decision 194; the slot is taken right after it, once the
inputs are checked) until it ends, however it ends: a `task run` through its worker and its reviewer (and a `task run`
list through every target), a `task start` through all its workers, a `sbxm run` through all its contestants, judge
and Jev sandboxes. Inside one sbxm process the slot is taken once: a command that already holds it (a `task run` going
on to its review, the template rebuild a task starts) never waits on itself. (3) **Which commands:** every one that
creates a **local** sandbox: `task run`, `task start`, `task review` (`--pr` too), `task fix --pr`, `task resume`,
`task gates` when it makes the worker's sandbox again, `sbxm run`, `template build`. Not a cloud sandbox (`--cloud`,
`location = "cloud"`, a task recorded `cloud`), and not `new`/`open` (long-lived interactive sandboxes). **`task loop`
takes it per task,** through the task's own commands, never for its whole run, so a command started by hand runs
between two of the loop's tasks. (4) **Where:** lock files `slot-<k>.json` (k in 1..=N) in the per-user local data
folder from the `dirs` crate, `dirs::data_local_dir()/sbxm/slots/` (`%LOCALAPPDATA%\sbxm\slots\` on Windows), never
under a base dir or the config dir, so two base dirs or two config dirs on one machine share them. Each names its
holder: the process id and start time (so a reused pid isn't mistaken for it, decision 163), the command line, the
task id when it has one, and since when. Written with `create_new`, with the Windows "Access is denied" retry of the
task lock (issues 407, 441), and given back only while it still holds what this command wrote. (5) **Waiting:** a
command with no free slot writes a ticket in `queue/` (named by its arrival time) and looks again every 5 s; it takes
a slot only when fewer older live tickets wait than slots are free, so waiting commands start in arrival order. It
prints `waiting for a sandbox slot: held by <command> (task <id>, process <pid>, since <time>)` once, and again only
when the holder changes (`<command> ... is waiting ahead of it` when the slot is free but an older command is in
line). **`--no-wait`** refuses instead, exit 1, naming the holder, writing nothing; it takes a free slot only when the
commands already waiting leave one over, by the same rule. (6) **A gone holder:** a slot whose process is gone (or
whose pid now runs with another start time) is taken over at once with one line naming the dead holder; a slot file
still unreadable at the next look (a command that died writing it) is taken over the same way; a gone waiter's ticket
is dropped. (7) **Ctrl-C while waiting** removes the command's ticket (and any slot it holds) and exits 130, having
created nothing; in `task loop`, whose own handler is installed first, its second Ctrl-C does the same. (8) **`sbxm
status`** shows one line when `sandbox_slots` is set: `sandbox slots: <held> of <N> held[ by <holder>; ...][, <n>
waiting]`, read only.
