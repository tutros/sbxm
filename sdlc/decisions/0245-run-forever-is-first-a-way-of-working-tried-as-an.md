245. **"Run forever" is first a way of working, tried as an experiment (2026-10-23, the user's eight answers; the user
asked on 2026-10-22 to run continuously while paid tokens last; replaces the overnight list-run trial, which ran two
of its planned 2-3 nights):** (a) a Claude Code session keeps a queue and runs `task run` one task after another,
triages stops and opens PRs; the user merges; no sbxm code, and an `sbxm task loop` feature is designed only from
the trial's numbers; (b) the queue is the open issues labeled `ready-for-worker`, a label set only once an issue's
open points are answered with the user (decision 244's boundary question included) and its size is confirmed; the
user's named order wins, otherwise size first (S, then M, then L) and, within a size, the oldest issue by creation
date first (PR 292 review Q-1, the user's choice); (c) an empty queue stops the
loop with a report of the almost-ready issues, the questions blocking each and a recommended answer, gathered read-only:
no new PRs, no sandboxes, no decisions taken for the user; (d) an environment stop (free disk, `sbx` not answering,
the usage limit, a killed process, a flaky gate) has its cause cleared where possible and the task resumed once, and
is parked on a second failure; a clean task is finished into a PR; should-fix findings alone are filed as sized
issues without the label; a work stop (must-fix left, a repeat finding, rounds used up) is finished into a draft PR
listing them and goes into the report, with no hand fixes inside the loop; (e) an issue whose `Depends on:` names an
unmerged issue or PR is skipped, everything else starts from `main`, a PR a merge left in conflict is reported, not
rebased, and the loop pauses while 5 of its PRs wait for the user; (f) the loop runs only the commands of narrow
allow rules the user adds (the rule text is written for the user to review; never edited by the agent), everything
else still prompts; (g) one task at a time end to end, each worker sandbox removed once its commits are saved, a
pause and a report if `sbx`'s drive stays under `min_free_disk` after that, the usage limit waited out, no cap beyond
the plan's own; two or more tasks at a time are an experiment for after the machine's upgrade (the NVMe drive; G61);
(h) each task's issue, size, rounds, outcome, time to ready, stops by cause and duration, the user's interventions and
the loop's idle time are logged in `sdlc/batches/<date>-loop.md` (unknown where not measured); the loop stops on the
user's word, an empty queue, the cap, two environment failures in a row, or anything security-related out of the
ordinary (an unexpected host, a secret in output, an agent touching files outside its task); the trial is 3 days or
15 tasks, whichever comes first, then an assessment decides about the feature.
