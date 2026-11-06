234. **A remade worker sandbox is checked at the delivered branch's tip (2026-10-20, milestone 3 slice 8 review Q-2,
the user chose it; narrows [216]):** when `task resume` makes a gone worker sandbox again, the template's lock files
are compared with the delivered branch's tip, not with the worker's starting commit as [216]'s "a resumed worker the
same one" said. The two differ only when the worker had already committed; the remade sandbox mounts the clone at
that tip and also runs the fix rounds, which [216] already checks at the tip. A strict profile is refused for a
`working (failed)` rerun whose own commits changed a lock file (gap G1).
