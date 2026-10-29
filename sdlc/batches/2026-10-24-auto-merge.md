# Auto-merge experiment log (decision 266)

Decision 266 lets GitHub's auto-merge merge a low-risk PR (the reviewer's level and the path-rule floor) without the
user, once the latest sbxm review covered exactly the PR's head commit with 0 must-fix findings and the required
checks pass. Auto-merge is armed with `--match-head-commit` set to that reviewed commit, so GitHub refuses a moved
head, and a later push to the PR disarms it first (`gh pr merge <N> --disable-auto`), with the new head reviewed
before it is armed again. The measure, over the next 10 auto-merged PRs: the breaks found after a merge (CI red on
`main`, a revert, a later must-fix traced to one of them) and the time from the PR being opened to its merge,
GitHub's `createdAt` to `mergedAt`, against the loop trial's median for the same interval, 29m12s
(`sdlc/retros/2026-10-23-retro.md`). Stop rule: the first break pauses auto-merge and the session tells the user,
who decides.

| # | PR | Risk | Head reviewed | Opened | Merged | Open to merged | Breaks found later |
|---|---|---|---|---|---|---|---|
| 1 | #326 (decision 269, docs) | low | f485501 | 2026-10-29 06:26Z | 2026-10-29 06:49Z | 22m56s | none so far |
