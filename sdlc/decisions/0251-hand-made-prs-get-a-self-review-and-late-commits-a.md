251. **Hand-made PRs get a self-review, and late commits a re-review (2026-10-23, the second retro's proposal 3):** a PR
made in the session, not by a worker, goes through the worker's self-review checklist ([188], `sdlc-implementation`)
before its independent review (`task review --pr N`); a commit pushed after a PR's last review gets a narrow re-review,
or the PR names it as unreviewed. An experiment for the next three hand-made PRs, dropped if they still need a fix
round; its measure is the hand-made PRs with a must-fix or a question in review round 1 (the second retro's window: 3
of 3, #289, #292, #311) and the commits merged after the last review (2, #304 and #311). In `AGENTS.md`'s "Workflow".
