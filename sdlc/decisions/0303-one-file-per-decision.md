303. **One file per decision (2026-11-06, the user's answers on issue #433; uses [244], [292], [296]):** the most
common conflict between PRs built side by side was two of them appending a decision at the end of
`sdlc/decisions.md`; reserved numbers (gap G41) stopped number clashes but not the append. So every numbered decision
is now a file of its own, `sdlc/decisions/NNNN-<slug>.md`: the number padded to 4 digits so the folder sorts by
number, and a short lowercase slug of its title (`[a-z0-9-]`, at most 50 characters) so GitHub's folder view lists the
titles. The file holds the entry as before; its first line is the title line. The folder is the index: nothing
generated is checked in, since a checked-in index would get the same conflict at its last row. Decisions 1-301 were
moved once, mechanically, with their text unchanged. `sdlc/decisions.md` keeps the round headings with their intro
lines and one line naming each round's decisions, "Open research spikes" and "Spike results"; its top says how to add
a decision: a new file with the next free number (one above the highest file, or the number an issue reserved).
`tests/decisions.rs`, a cargo test of its own (decision 292) so the worker gates and CI run it with no new step, checks
that every file name matches `^\d{4}-[a-z0-9-]{1,50}\.md$` with the number on its first line, that no number is used
twice (gaps are allowed), and that every reference to a decision in the tracked `.md` and `.rs` files ("decision N",
"decisions N, M", "decisions N-M", or `[N]` in `sdlc/`) resolves to a file, except an allow-list of numbers known to
be unwritten (168, 271), each with its reason; a failure names the file, the line and the number. Rust, not
PowerShell: the worker sandbox has no `pwsh`. A PR that adds a decision to `decisions.md` while this change is open
gets it moved into the folder when this one merges `main`.
