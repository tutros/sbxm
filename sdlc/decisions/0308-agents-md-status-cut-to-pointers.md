308. **AGENTS.md's Status section cut to pointers (2026-11-08, the retro, proposal 3, the standing removal of [287]; an
experiment):** the Status section was 8.3 KB of AGENTS.md's 21 KB, loaded by every session and worker, and stale (#58
and #60 listed open, both closed; M4 "built on branch m4", merged as PR #322). Assumption tested: agents need the
milestone history in always-loaded instructions. **Change:** one paragraph naming the merged milestones and pointing to
`docs/project.md` and #405; "Sources of truth" kept. **Trial:** the next 3 tasks. **Measure:** minutes to first ready
and round-1 must-fix per task, by size (window: S 26.5 min and 0.33; M 44.4 min and 1.73); restore the section from git
if both get worse.
