292. **`task fix --pr N` merges the base first, and two rules avoid conflicts (2026-11-03, the user's answers on issue
#404; uses [251]):** open PRs conflicted with each other once one merged (3 of 6 on 2026-11-03), and every conflict was
mechanical. (1) The fix task's worker prompt starts with "merge the base": `git merge --no-edit origin/<base>` in the
sandbox, each conflict resolved by keeping both sides whole (both appended tests, both appended decisions; a README
row both sides edited gets both edits, word by word), never a rebase, reset or force (the push is a fast-forward); a
conflict that needs a judgement about behavior is aborted and handed back. A delivered branch that doesn't contain the
base's tip as the task fetched it is not pushed. The gates then run as for any task (a resolution that breaks a test
fails them and gets a fix round), and the in-task review, posted on the PR as before, covers the merge narrowly: the
reviewer prompt says the base's own commits are not the change and to check the merge commit's resolutions
(`git show --remerge-diff`). A clean merge needs no fix round. A PR whose base moved twice is merged again by the next
`task fix`. A head-covering review with 0 must-fix is accepted when GitHub reports the PR conflicting with its base or behind it (`PrInfo::base_moved`, from `mergeable`/`mergeStateStatus`); any other PR with 0 must-fix is refused before anything is made. (2) Fewer conflicts at the source, as rules in the worker prompts and `sdlc-issue-flow`: (a) an issue
reserves the number of any decision it adds when it is readied ("this adds decision N", gap G41); (b) new tests go in
a section or file of the issue's own, never appended at the end of a shared test file. README rows edited by two PRs
get no rule. (3) The loop doesn't avoid tasks whose files overlap an open PR's; the trigger to build that is a
worker's merge of the base failing its gates twice in one week.
