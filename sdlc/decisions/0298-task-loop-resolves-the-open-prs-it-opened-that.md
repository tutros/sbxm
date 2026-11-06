298. **`task loop` resolves the open PRs it opened that conflict with `main` (2026-11-05, the user's answers on issue
#432; uses [266], [280], [292], [293]):** each merge to `main` could leave other open PRs conflicting, and each then
needed a hand merge (#404: 5 on 2026-11-03). (1) Conflicting only: a PR is resolved when GitHub reports `mergeable`
`CONFLICTING` or `mergeStateStatus` `DIRTY`; a PR only `BEHIND` still merges (no branch protection on `main`), so it
is left, and `UNKNOWN` is read again at the next step. `PrInfo::base_moved` counts `BEHIND` too, so the loop reads
its own field (`PrMergeState::mergeable`). "Require branches to be up to date" stays off; trigger to revisit: the
first break on `main` from two PRs that each passed alone. (2) At every loop step, after `task rm --merged` and before
the next issue (and before the pause on 5 PRs, since a resolve may free a place), each open PR a loop opened is read
and the conflicting ones get `task fix --pr N`, one at a time in PR-number order; at most one resolve per (PR, tip of
`main`), in memory only. Resolves don't count toward `--cap`; each gets its own log row. (3) Auto-merge, if armed, is
disarmed before the fix (`gh pr merge --disable-auto`), and armed again with `--match-head-commit` only when the fix
task's review is low risk after the path-rule floor, has 0 must-fix and covered the PR's new head; and only for a PR
that was armed before, or whose own task and every resolve already pushed onto it (its finished fix tasks' records)
passed [293]'s checks, so a narrow low-risk review of a merge never arms a PR whose own review, or an earlier
resolve's, was medium or high. Drafts are skipped. (4) A failed resolve (a hand-back, failing gates, must-fix
left) leaves the PR and its fix task for the user, counts toward the cap of 5, is said once and skipped for the rest
of the run; the loop goes on and never removes a fix task.
