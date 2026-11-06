244. **Boundary tests first, in place of the edge-case checklist (2026-10-22, the first retro's proposal 3):** when an
issue's open points are answered, one question replaces the edge-case checklist tried since 2026-10-14 (its tasks
had a round-1 must-fix as often as before, 3 of 6): does the change have a limit, a size, or an identity (path,
drive, ref) that decides behavior? If yes, the issue body lists the boundary cases (at the limit, one over, one
under, the largest input) and the worker writes those tests first. An experiment for the next three issues that
answer yes; its measure is their round-1 must-fix and rounds (window 3's boundary changes: #244 7 rounds, #258 5,
#285 4). In `AGENTS.md`'s "No open questions go to a worker" rule.
