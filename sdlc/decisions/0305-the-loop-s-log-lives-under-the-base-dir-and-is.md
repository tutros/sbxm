305. **The loop's log lives under the base dir and is posted to the open `loop-log` issue (2026-11-08, the user's
answers on issue #463; changes [245](h), uses [244], [303]):** `task loop` appended its rows to
`sdlc/batches/<date>-loop.md` in the checkout it ran from, and those logs are committed, so every loop left a tracked
file modified (a build refused on it on 2026-11-06), and a second loop the same day numbered its rows from 1 again in a
file of the same name `main` already had (2026-11-07; the rows needed a spec, a review and PR #494 to land). (1)
**One file per loop, never in a checkout:** `<base_dir>/.sbxm/loop-logs/<date>-<HHMMSS>-loop.md`, next to the loop's
lock, named by the loop's start in UTC with seconds, rows numbered from 1; "an issue already logged is skipped" holds
within that file. (2) **Posted at the stop:** the whole log goes as a comment on the open issue labeled `loop-log`, the
lowest-numbered when there are several; with none, the loop creates `Loop logs since <date>` (the start date, UTC),
labeled `loop-log`, with a one-line body, then posts to it. One issue per retro window, so none grows without bound:
the retro reads it, links it from its record and closes it. The comment starts with the loop's start time, its stop
reason (the error's first line for a stop on an error) and the local file's path, then the table. (3) **A comment over
65,000 characters** (GitHub caps one at 65,536) is posted as several, split between table rows and never inside one,
each repeating the heading line and the table header, marked `(part i of n)`; a row too long on its own goes alone in
its part. (4) **A post that fails** (no network, `gh` not signed in, the label missing, a create or a comment refused)
is one warning naming the local file and the error; the loop's exit code and stop message don't change, and nothing is
retried. A loop that ran no task posts nothing; a second Ctrl-C (exit 130) posts nothing either, and its local file
stays (a known limit). (5) The committed `sdlc/batches/*-loop.md` stay as history; `sdlc/batches/README.md` says where
the logs are from 2026-11-08 on.
