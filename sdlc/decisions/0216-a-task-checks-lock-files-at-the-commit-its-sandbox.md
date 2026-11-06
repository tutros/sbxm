216. **A task checks lock files at the commit its sandbox checks out (2026-10-15, PR 253 review round 2 Q-6, the
user agreed with the recommendation; changes the base-commit rule of [206]):** issue and spec workers use their
starting commit (a resumed worker the same one); a continued PR's worker the PR head it starts from; `task review
--pr` the fetched PR head; an in-task reviewer and fix rounds the delivered branch's tip. All are read from the
task's `repo.git` [159]. A failed check refuses a strict sandbox before it is created, naming `template build
--warm`. Why: a PR that changes a lock file would otherwise pass a check made at its base and fail later in the
gates with missing-dependency errors. A worker changing a lock file mid-task stays gap G1 [203].
