219. **A risky slice's parallel review reads in parallel but builds in turn (2026-10-16, PR 263 review Q-1, the user
chose option A; refines [162]):** the independent reviewer of a risky slice still starts as soon as the slice is
committed and reads the diff while the next slice is built, but its own compile, `cargo test` and other gate runs
wait until no slice build is running, and the builder doesn't start a new build while those checks run. A reviewer
on separate capacity (not this machine) can run its checks at once. Why: two local builds of the crate at the same
time both died with no error (gap G48, 2026-10-14), and running one build at a time has worked since. Rejected:
reviewing the whole slice before the next one starts (B), which loses [162]'s early feedback without need, and
accepting parallel builds that may fail and be rerun (C).
