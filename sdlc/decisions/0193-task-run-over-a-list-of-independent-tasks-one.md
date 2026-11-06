193. **`task run` over a list of independent tasks, one after another (2026-10-13, decided with the user before
issue 201 started, one point at a time, recorded on the issue):** each evening's work was a host command per task,
each waited for and started by hand; a list cuts that to one command. (a) **A stopped task does not stop the
list.** A task that ends short of `ready` (must-fix findings left, a failed worker, a gate that could not run) is
recorded and the list goes on to the next one. The list stops only for a problem that would hit every task: `sbx`
not reachable, a missing secret, too little free disk. The tasks are independent: a later task never gets an
earlier task's work, and the docs say so. (b) **No disk check of its own.** The check is issue 210's, which was
built first; here it runs before every task of the list, not only at the start, since a task's own sandbox can fill
the drive the next one needs. A failure is a list-wide stop: the tasks done so far keep their results and the
summary names the ones not started. (c) **One task at a time.** No `--parallel`; parallel runs become their own
issue once a sequential list has proved too slow in practice. (d) **Exit code.** 0 only when every task of the list
ended `ready` (as for one task, `ready` with must-fix findings left is reported, not an error). 1 when at least one
task failed or was not started because the list stopped; the summary line per task carries the detail. (e) **No
allow rules** (the point moved from issue 203): `task start`, `review`, `run`, `resume` and `gates` stay on an
approval prompt, since one command per list leaves little to save and a rule cannot exclude `task review --pr`
(posts a comment), `--restart --yes` (discards a task) or host gates. (f) **How the list is written.** Repeated
flags, run in the order written: `sbxm task run --issue 210 --issue 202 --spec sdlc/specs/x.md`; no list file. The
other flags (`--worker-harness`, `--worker-model`, `--reviewer-harness`, `--reviewer-model`, the time limits,
`--profile`, `--base`, `--repo`) apply to every task of the list. `--restart` is refused when the list has more
than one task: there is nothing left to show the user when several tasks' old copies would be discarded at once.
Implementation note: clap's derive API carries no field for the relative order of two different repeated flags, so
`--issue`/`--spec` order is read back from the parsed command's raw `ArgMatches` (`cli::run_target_order`).
