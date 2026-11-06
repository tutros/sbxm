280. **Low-risk PRs keep merging without the user (2026-11-02, the user's choice; ends the experiment of [266]):**
the 10 PRs of [266] (c) auto-merged with no break found, in a median 21m42s from opened to merged against the
baseline's 29m12s (`sdlc/batches/2026-10-24-auto-merge.md`). [266] (a), (b) and (d) stay as written: low risk only
(the reviewer's level and the path-rule floor), the three checks and `--match-head-commit` with the reviewed SHA,
disarming before any later push, and the GitHub settings; medium and high risk stay the user's. Not kept: the
per-PR log, which ends with row 10. Kept from (c): a break found after an auto-merge (CI red on `main`, a revert, a
later must-fix traced to one) pauses auto-merge, and the session tells the user, who decides. Since 2026-11-04 GitHub enforces the reviewed-head rule itself: `sbxm/review` is a required check in `sbxm-ruleset` next to the two `build` jobs, so a PR, auto-merged or not, merges only once an sbxm review has posted success for its current head.
