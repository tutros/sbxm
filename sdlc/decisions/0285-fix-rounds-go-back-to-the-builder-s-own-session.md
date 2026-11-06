285. **Fix rounds go back to the builder's own session (2026-11-04, the fourth retro's proposal 1, an experiment):** a
fix round of a Claude worker resumes the worker's session instead of starting a fresh one, so the builder's reasoning
carries over (pattern 1 of `sdlc/retros/2026-11-04-retro.md`: 4 of 9 tasks with a fix round needed a second one). The
code is issue #418 (size M, open points first). Trial: the next three tasks with a fix round after it merges; measure:
tasks needing a second fix round (4 of 9), repeat must-fix (4 in 3 of 10 tasks), the fix round's cost.
