297. **A session watches CI after every push and arm, and acts on a failure at once (2026-11-06, the user's choice;
an experiment):** PR #440 (#92) had auto-merge armed when its Windows job failed on a test that hard-coded the Linux
drive wording; nobody looked until the user asked about CI. Main's failed run of PR #437's merge (the lock read race,
#441) waited the same way. So `sdlc-issue-flow` stage 7 now starts a background watch after every push to a PR and
every arm, and after a merge on `main`'s run of the merge commit; a failure's log is read the same turn, a runner
fault or known flake gets one re-run of the failed job (and an issue if new), a real failure on an open PR is fixed
on its branch with auto-merge disarmed first and a narrow re-review, and a real failure of the merge commit's run on
`main` is a break after the merge, handled by decision 280's pause (the user decides), never a push to the merged PR. This only works while a session is open; the
unattended form is `task loop` reacting to its PRs' checks (#442), which polls GitHub now.
A push-based event bus (webhooks or pub/sub, an agent invoked per event, code deciding the fixed reactions) is a
direction, not built: trigger, more than one machine or several cloud workers at once, or polling delay costing real
time. Measure: minutes from a check failing to the first action on it, over the next three PRs (PR #440: not
measured; until the user asked). Judged at the next retro.
