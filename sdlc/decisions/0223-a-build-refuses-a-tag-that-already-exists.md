223. **A build refuses a tag that already exists (2026-10-17, milestone 3 slice 4, announced to the user; adds to
[211]):** before any write, a build whose generated tag is already in `sbx template ls` (a second build in the same
minute) is refused: "template TAG already exists; wait a minute and build again". Why: `sbx template save` replaces
an existing tag silently (S11, question 8), and the tag has minute resolution.
