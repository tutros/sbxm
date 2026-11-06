295. **A ready issue is re-checked right before its task starts (2026-11-05, the user's choice; decisions 290-294 are
reserved for the two groups):** a `ready-for-worker` label goes stale when code merges under it. On 2026-11-04 three
labeled issues no longer matched the code or themselves: #327 (new open points from code merged after it was
readied), #92 (its acceptance list contradicted its answers) and #376 (it assumed a loop-lock format the code
doesn't have). So `sdlc-issue-flow` stage 2 now re-checks a ready issue before its task starts: the body read back,
what it names checked against `main`, the PRs merged since its label listed, and its acceptance list checked against
its answers; a point that no longer holds goes back to the user and the label comes off until it's answered. The
automated version is a third job of the scheduled check (#327). Rejected: a date check in `task loop` alone, which
can't tell a stale issue from a valid one. And issues are readied just in time (the user, 2026-11-05): only those
for the next session or night run, not the whole backlog, so fewer points reopen as the code changes.
