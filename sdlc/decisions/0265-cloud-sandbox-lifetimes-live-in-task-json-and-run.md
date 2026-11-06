265. **Cloud sandbox lifetimes live in `task.json` and `run.log`; removal is unchanged (2026-10-24, the user's answers
to milestone 4's P-choices; P7 (a) kept, P18 and P19 dropped; builds on [195], [259]):** `task.json` keeps an
append-only `sandboxes` list (name, role, location, created_at, ttl_s, removed_at), and `run.log` gets one line at
each cloud create and each removal. They survive as every other retro number does today: `task rm --merged`
archives `task.json` and `run.log` [195]; a plain `task rm` loses them, as it already loses rounds and times. No
change to `task rm`, `--restart` or the archive layout, so P18 (archiving on every removal) and P19 (an archive per
task attempt) are dropped. Grounds: merged tasks, almost all of them, are archived already; changing removal for a
number nobody has needed yet goes against optimizing just in time. Gap G62 in `sdlc/evals-workflow-notes.md` names
the trigger for revisiting it.
