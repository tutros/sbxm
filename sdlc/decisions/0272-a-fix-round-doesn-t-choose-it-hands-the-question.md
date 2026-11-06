272. **A fix round doesn't choose; it hands the question back (2026-10-30, the third retro's proposal 1,
`sdlc/retros/2026-10-30-retro.md`; extends [188]):** when fixing a finding needs a fact about an external tool's
behavior that can't be checked from inside the sandbox, or a choice between behaviors that the issue, the decisions
and the review don't settle, the fix round leaves that finding unfixed and writes it under "Needs a decision" in
`result.md`, with the options. The rule is a bullet in `prompts/fix.md` and `prompts/fix-spec.md`. An experiment for
the next three tasks with a fix round; its measure is repeat findings per task (the third retro's window: 3 in 2 of
10 tasks, #314 and #330).
