249. **A full queue before a loop window (2026-10-23, the second retro's proposal 1, `sdlc/retros/2026-10-23-retro.md`;
amends [245]'s start):** before a loop window starts, at least as many issues as the window's task count are labeled
`ready-for-worker`, M issues included where any are decided, and a merge time is agreed with the user so the cap of 5
PRs awaiting the user isn't reached while the user is away. An experiment for the next loop window; its measure is the
share of the loop's idle time (one task's `ready` to the next task's start) spent waiting for the user (the first
window: 56%, 1h57m48s of 3h30m28s) and tasks per hour of the window's span (2.6).
