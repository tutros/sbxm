293. **Only `task loop` arms auto-merge (2026-11-05, the user's answers of 2026-11-02 on issue #375; part 2 of #320):**
after a task's `task finish`, the loop runs `gh pr merge <N> --auto --merge --match-head-commit <reviewed sha>` on
the PR it opened, under [266] and [280]: the risk after the path-rule floor is low, the in-task review covered the
PR's head with 0 must-fix, and the PR isn't a draft. (1) No CI check before arming and no `gh pr checks` polling:
GitHub's `--auto` waits for the required checks, `sbxm/review` among them. (2) Only `task loop` arms; `task finish`
and `task review --pr` don't, and for a PR made by hand the session arms under [280]. Everything else waits for the
user and counts toward the cap of 5: medium and high risk, a draft, a head that moved after the review (the loop's
log row says so), and an arming call that fails (a warning; the loop goes on). A PR already armed is left alone. A
merged PR's task is removed by `task rm --merged --yes`, which the loop runs before each step. The count of PRs
awaiting the user is one function (`queue::awaits_user`), shared with the Loop section of `sbxm status` [294].
