331. **Readiness rules as code, and overlapping work holds before review (2026-11-18, the user's answers; builds on
[296], [295], [298], [244] and [269](c)):** PRs #720 and #722 conflicted and were resolved side by side, because
decision 296's same-spot check is done by hand and was skipped when eight issues were labelled together. (1)
`sbxm task ready-check --issue N` prints a pass or a fail for each readiness rule, plus an LLM's notes; the
deterministic rules are code (one size, line and label agreeing; well-formed `Depends on:` and `Real check:` lines;
no `Open:` or "to decide" markers left; an acceptance list; the paths the body names exist on the default branch;
the PRs merged since the label that touched those paths, decision 295, as a warning); the loop runs these before it
starts an issue and skips a failing issue, with the reason; (2) the judgment rules (unanswered points in the prose,
boundary cases under [244], the acceptance list against the answers, a proposed `**Files:**` line, the size's fit)
go to one model call in a fresh sandbox on the headless runner, which reaches only the model provider ([269](c):
issue text is untrusted) and answers in a fixed format; it advises, and the label stays the user's; a `**Files:**`
line is advice, never required; (3) overlap is checked after the work, on the real diff, not predicted: a task whose
changed files, after its gates pass, overlap an open loop PR's changed files, a running resolve or fix on a PR, or
another task already in review, is held before its review, without a slot, while the loop starts other work; when the
overlapping work has merged or ended, it merges the default branch (an agent resolves any conflict, as `task fix`
does under [298]), runs its gates again, and only then gets its review, so an overlapping pair costs one review, not
a review, a resolve and a second review, and the user never sees a conflicting loop PR; the task held first goes
first; this replaces [296]'s manual check for loop work (it stays for issues run by hand). Measure, judged at the
next retro: the holds (count, and the minutes each waited, split into waiting on the user's merge and waiting on a
task), the holds whose merge of the base had a conflict (the PRs that would have conflicted), the reviews and
resolves per merged PR against this window, and open loop PRs that still conflicted with `main`.
