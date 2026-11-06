242. **No parked should-fix findings at a milestone review (2026-10-22, the first retro's proposal 1,
`sdlc/retros/2026-10-22-retro.md`; changes how [162]'s slice reviews end):** a slice review's should-fix findings
are fixed in the next slice or filed as an issue before the milestone review starts, and the milestone PR lists
each filed one with its issue number, so the review doesn't raise it again. The reviewer's half (PR 290 review M-1):
the end-of-milestone reviewer is given that list (from the slice reports, before a PR exists) and records an
unchanged listed finding under `## Already filed` instead of as a new finding; a changed or worse one is reviewed as
new, naming the issue it supersedes (`sdlc-code-review` section 1). An experiment for the next milestone;
its measure is the number of findings at the milestone review that a slice review raised first (milestone 3: 10, 3
of them as must-fix) and the milestone review's rounds (milestone 3: 3).
