237. **`sbxm run` chooses one template per profile and harness in its preflight (2026-10-21, milestone 3 slice 9,
the user agreed with the recommendation; adds to [206], [216]):** before any write, the run's preflight resolves the
template of every (profile, harness) pair a contestant or the judge uses, once, and that choice holds for every
sandbox of the pair. A warm template's lock files are checked against the seed folder, which every contestant's
workspace copies; a run without a seed has no lock files, so a warm template counts as stale (a strict profile is
refused, a non-strict one warns and installs), while a cold template is always usable. The judge follows its pair's
choice: it builds nothing, so it gets no lock check of its own.
