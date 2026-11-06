296. **Issues that edit the same spot run in order, never side by side (2026-11-06, the user's choice; an
experiment):** open PRs conflicted more often once night runs and grouped tasks put several PRs up at once. PR #437
(#375, #376) conflicted with PR #434 (#91): both added a section to `sbxm status` in the same three files. PR #426
(#402-#404) conflicted with main on the module list in `src/task/mod.rs` and the end of this file. So `sdlc-issue-flow`
stage 2 now names the files and spots an issue will edit, and compares them with the open PRs and the other issues of
the same run: two that edit one spot go to one spec (decision 286) or run one after the other. Decision 292 (new
tests in a file of their own, decision numbers reserved) and #433 (one file per decision) remove the shared-file
spots; this check covers the code. #404 (`task fix` merges the base) and #431/#432 resolve what still conflicts.
Measure: open PRs that conflicted with `main` at review or merge time, per merged PR, over the next ten merged PRs
(this week: PRs #426 and #437 conflicted, #428 and #434 didn't). Judged at the next retro.
