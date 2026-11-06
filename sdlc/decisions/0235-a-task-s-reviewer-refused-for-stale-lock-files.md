235. **A task's reviewer refused for stale lock files says how to go on (2026-10-20, milestone 3 slice 8 review Q-1,
the user chose option a; adds to [216]):** when a strict profile refuses an issue or spec task's reviewer because
the lock files at the delivered tip differ from the warm template's, the refusal keeps [216]'s `--warm` hint and adds
the command that continues the task (`sbxm task resume --issue N`, or `--spec <file>`). When the lock files at the
task's base still match the template, it also says the change came from the task's own commits (gap G1), that
`strict = false` in the profile lets the review install them instead, and the tip commit to warm a template from.
A PR task's refusal is unchanged.
