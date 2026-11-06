287. **Each retro removes one rule; first, AGENTS.md's code layout (2026-11-04, the fourth retro's proposal 3, an
experiment):** `sdlc-retro` section 4 now asks every retro to propose removing one rule or harness component, naming
the assumption it encodes. This retro's: the "Code layout" section of `AGENTS.md` (31 KB of 50 KB, loaded by every
session and worker) became one paragraph; the assumption tested is that agents need a prose map to find code. Trial:
the next three tasks; measure: worker time to first `ready` (median 20.5 min) and round-1 must-fix per task (0.73);
the section is restored from git if both get worse.
