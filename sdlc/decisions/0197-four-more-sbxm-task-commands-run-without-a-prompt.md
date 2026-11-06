197. **Four more `sbxm task` commands run without a prompt (2026-10-14, the user's decision in the session that
day; changes [193](e) and adds to [192]):** `task resume`, `task finish`, `task rm --pr` and `task rm --merged` are
allowed through the wrapper of [192](a), with the rules in `.claude/settings.local.json`, added by the user. Why:
waiting for an approval was part of every gap between commands (issue 202: about 2 h 16 min of machine time over
14.5 h), and these four only continue, publish or clean up work the user already started: `task finish` acts only on
a `ready` task and merging stays the user's; `task rm --pr` deletes a reviewer's copy whose review is posted on the
PR; `task rm --merged` removes only merged tasks and archives their records first [195]. Still on approval: `task
start`, `task review`, `task run` and `task gates` (they start new work or post a review), and `task rm --issue` and
`task rm --spec` (before `task finish`, a worker's result exists only in its task). A rule matches only the command
called on its own, so an agent runs these commands alone, not chained with others.
