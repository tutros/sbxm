# Auto-merge experiment log (decision 266)

Decision 266 lets GitHub's auto-merge merge a low-risk PR (the reviewer's level and the path-rule floor) without the
user, once the latest sbxm review covered exactly the PR's head commit with 0 must-fix findings and the required
checks pass. Auto-merge is armed with `--match-head-commit` set to that reviewed commit, so GitHub refuses a moved
head, and a later push to the PR disarms it first (`gh pr merge <N> --disable-auto`), with the new head reviewed
before it is armed again. The measure, over the next 10 auto-merged PRs: the breaks found after a merge (CI red on
`main`, a revert, a later must-fix traced to one of them) and the time from ready to merged, against the loop trial's
median of 29m12s. Stop rule: the first break pauses auto-merge and the session tells the user, who decides.

| # | PR | Risk | Head reviewed | Ready | Merged | Ready to merged | Breaks found later |
|---|---|---|---|---|---|---|---|
