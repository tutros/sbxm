307. **A loop check before handing over a loop command (2026-11-08, the retro, proposal 2; an experiment; uses [245],
[295]):** in 2026-11-05..08 the loop met problems only at run time: #485 was passed over (its `**Depends on:**` line
named the open #476 as "Related"), #468 was refused (branch `issue-468` already on origin), #476 had the same Depends
problem; and commands handed to the user were wrong three times. **Change:** `sbxm task loop --check [--issue N]...`
prints, per issue, `take` or `skip: <reason>` from the loop's own pre-checks, then the exact loop command, and runs
nothing (#511); `sdlc-issue-flow` section 2: hand the user only the command it prints. **Trial:** the next 3 loops.
**Measure:** skips and refusals met at run time (window: 2 in 7 loops), wrong commands given to the user (window: 3).
