330. **The loop fires recurring work: a retro alert and the improvement review (2026-11-17, the user's five answers;
issue #152's trial, PR #705, filed 6 of its 9 findings; supersedes [269]):** (1) the trigger lives in the loop, which
always runs: on each idle check it works out what is due and acts, each once per threshold; decision 269's script
skipped every run while the loop held its lock, so it never ran (issue #704); (2) an improvement review is due after
20 merged PRs since the newest `sdlc/reviews/*-improvement-*.md` (not weekly, so a quiet week costs nothing): the
loop queues the review as a spec job, should-fix findings and questions only, nothing that blocks a PR, and the user
sorts its findings; (3) each review covers one area from the list in `sdlc/reviews/areas.md`, the area whose newest
review is oldest going next (an area never reviewed first); the review's file name ends in the area's slug
(`<date>-improvement-<slug>.md`), and the list changes by a docs PR with no rebuild; (4) a retro is due at 10 merged
PRs since the end date of the newest retro's `Window:` line: the loop shows a toast, comments on the open `loop-log`
issue and adds `retro due: N PRs since <date>` to `sbxm status`, then shows one toast a day until a new
`sdlc/retros/*-retro.md` is on `main` (a retro needs the user, so the loop only reminds); (5) decision 269's
experiment is dropped: `scripts/factory-check.ps1`, `scripts/factory-trigger.ps1`, their Pester tests and the
`factory` profile are removed (the next retro's removal under [287]); #704 and #328 are closed; readiness drafts
become a gap with a trigger. Measure: the share of each review's findings the user files (the trial's was 6 of 9),
and the days from a retro due to its record; the next retro judges both.
