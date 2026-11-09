306. **Fix small should-fix findings in the same task (2026-11-08, the retro, proposal 1; an experiment; uses [195],
[245]):** in 2026-11-05..08, 5 should-fix findings were filed as new issues (#429, #441, #467, #480, #502). Each took a
median 16 min of agent time, about one fix round (the window's `task fix --pr` tasks: median 21.75 min to ready), but
also readying, a sandbox, a full review and gates, a PR and a merge (1 of 5 by the user), and a median 15 h from filing
to merge. **Change:** `[loop] should_fix = "fix"` (default `"file"`): when a task's review has 0 must-fix but should-fix
findings sized S, the loop runs one more fix round on that task before `task finish`; M or L findings, and any left
after that round, are filed as today. The code is #510. **Trial:** the next 3 tasks with an S-sized should-fix finding.
**Measure:** hours from finding to merged (15 h median), extra issues, PRs and merges per finding (1 each), agent
minutes per finding (16).
