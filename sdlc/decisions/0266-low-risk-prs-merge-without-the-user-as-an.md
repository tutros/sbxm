266. **Low-risk PRs merge without the user, as an experiment (2026-10-24, the user's four answers; narrows [86] and
[187], where the user merges):** (a) scope: a PR whose risk is low, both the reviewer's level and the path-rule floor
[176], [179]; medium and high stay the user's to merge, for now. (b) It starts now, as a process: the session checks
that the latest sbxm review covered exactly the PR's current head commit, found 0 must-fix, and rated it low, then
runs `gh pr merge <N> --auto --merge --match-head-commit <sha>`, where `<sha>` is the full SHA of the commit that
review covered (the end of its `Scope:` range); GitHub refuses the request if the PR's head is not that commit, and
otherwise merges once the required checks pass. A session never pushes to a PR whose auto-merge is armed: it first
runs `gh pr merge <N> --disable-auto`, then pushes, and the new head gets a review (`task review --pr N`) before
auto-merge is armed again with the new reviewed SHA. Why: GitHub keeps auto-merge armed when someone with write access
pushes, so the SHA guard alone only covers the moment it is armed. The `sbxm/review` status check that would make
GitHub enforce this is issue #317, built if the experiment works. (c) Measure: over the next 10
auto-merged PRs, the breaks found after a merge (CI red on `main`, a revert, a later must-fix traced to one of them)
and the time from the PR being opened to its merge, GitHub's `createdAt` to `mergedAt` (the loop trial's median
for the same interval: 29m12s, `sdlc/retros/2026-10-23-retro.md`); the user's first answer said "ready to merged"
and the user changed it to this interval on 2026-10-24 (PR 318 review S-2), because the baseline was measured from
the PR being opened. The first break pauses auto-merge and the
session tells the user, who decides. Every auto-merged PR is logged in `sdlc/batches/2026-10-24-auto-merge.md`.
(d) GitHub settings, made by the user on 2026-10-24: the ruleset `sbxm-ruleset` requires 0 approvals and no
code-owner review, requires the checks `build (ubuntu-latest)` and `build (windows-latest)` (CodeQL stays enforced by
its code-scanning rule), with 'require branches to be up to date' off (updating a branch adds an unreviewed merge
commit; a clash between two merged PRs shows as red CI on `main`, a break); auto-merge and automatic deletion of head
branches are on. Why: the user wants merges to happen without them; low risk only keeps the trust-boundary code with
them.
