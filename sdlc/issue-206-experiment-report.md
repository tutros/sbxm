# Issue 206 experiment: self-review and class fixes in the prompts

Written 2026-10-12, after the third of three tasks. The aim of issue #206 was no must-fix finding in the first review round. Issue #220 part 1 (worker limits as prompt text) was tried on the third task only.

## What was tried

The built-in prompts were edited on branch `issue-206-prompts` and the tasks were run with a binary built from it (the `[prompts]` overrides were too coarse, #217):

- `9bb5014` (#206): the worker reviews its own diff before it reports and writes a "Self-review" section; a fix round names the kind of problem and fixes every place it occurs.
- `3041ae4` (#220 part 1): three tiers of what a worker decides, declares or hands back, the result sections "Choices made" and "Needs a decision", the same exit for the reviewer, and "never end your turn while waiting on a background command" in the fix prompts. The two gate-fix prompts got the same limits and turn-end line only after the experiment (PR #227's review, M-1), so no run tried them.

## Results

| Task | Binary | Round-1 must-fix | In-task review rounds | Fix rounds | Ended |
|---|---|---|---|---|---|
| #207 (undoable `--restart`) | `21b2d76` (#206 text) | 1 | 7 | 4 | 1 must-fix left, then 2 PR review rounds and hand fixes; 2 findings filed (#223, #224) |
| #90 (`sbxm status` Tasks section) | `21b2d76` (#206 text) | 1 | 6 | 4 | clean, then a hand change (decision 186) |
| #198 (in-task review counts as the PR review) | `3041ae4` (#206 and #220 text) | 1 | 3 | 1 | clean in 37 minutes |

Baseline (issue #206, from 34 saved runs): 16 runs (47%) had a must-fix finding in round 1, 0.85 findings per run.

Baseline review rounds, counted 2026-10-12 from the same 34 logs (the highest `review round N` line of each run's log, in the local e2e folder, which is not in the repository):

- The 18 runs with a clean round 1 ended after that one round.
- The 16 runs with a round-1 must-fix took 2.5 rounds on average before the run stopped: 1 round once, 2 rounds seven times, 3 rounds seven times, 4 rounds once. Two of them were resumed afterwards and reached 6 rounds (#120) and 4 rounds (#194).
- The three experiment runs, counted the same way: 2 rounds (#207, resumed to 7), 2 rounds (#90, resumed to 6) and 3 rounds (#198).

The round counts don't compare cleanly. The older runs had one fix round at most, and the narrow and full re-reviews came later, so a run with one round-1 finding now takes at least 3 rounds (the finding, a narrow re-review, a full one). #198 took that minimum. #207 and #90 were well above the baseline once resumed. The conclusion below therefore rests on the round-1 count and on what the fix rounds did, not on the number of rounds.

## What it shows

1. **The aim was not reached.** 3 of 3 runs had a must-fix finding in round 1, against 47% before. Three runs can't show a change in either direction, but the self-review did not catch any of the three findings. Each time it listed what it had checked, and the finding was somewhere else (#198: copyable commands in hints that dropped the repo and the base).
2. **Fixing the kind worked once and failed once.** In #198 the fix round named the kind, put the command behind one helper, fixed four places (one of them older than the branch) and named a fifth it left alone; round 2 was clean. In #207 the same prompt didn't stop the rounds: the kind (a failure path of the restart's own recovery) came back in 5 findings until the structure was changed by hand. A fix agent names the kind the reviewer's finding shows, not the wider one behind several findings.
3. **The #220 sections were used and were useful (one run).** #198's result has nine entries under "Choices made" and one under "Needs a decision" (confirm decision 187). Nothing was left undone, so the hand-back path itself is untested. The largest choice, a new `review.commit` key in `task.json`, was declared and written into the decision instead of being found by the reviewer, which is what the section is for. The reviewer used no "Needs a decision:" question.
4. **The turn-end line held (one run).** #198's fix round committed its work; #90's had lost a round by ending its turn while a test run was in the background.

## Also seen

- #198's PR review (PR #226, the whole branch again) found 2 more must-fix findings, both where the review comment never reached GitHub. The worker's self-review had named one of them ("the suggested `gh pr comment` command posts `review.md` without the risk section") as a possible should-fix and left it. So the self-review can see a problem and still misjudge its weight.

- #198's round 3 was a full review by its prompt, but the reviewer wrote the fix commit's range as its scope and took 3 minutes: with `previous-review.md` present it treated the round as a re-review. The whole branch was reviewed in round 1 only. In #207 the full rounds did find new problems, so this is one case, not a rule.

## Recommendation

Adopt the branch as it is, with the aim reworded:

- Keep the fix-prompt text (the kind of problem, every place) and the turn-end line: one clear success, no cost.
- Keep the #220 part 1 text: the "Choices made" list makes the user's check of a result short. Part 2 (sbxm support) waits until a task hands something back.
- Keep the self-review section, but don't expect it to remove round-1 findings. In #198 it named two small should-fix items and changed nothing.
- Rounds that keep finding one kind are not solved by a prompt. The rule from #207 stands: after two rounds of the same kind, stop and change the structure with the user.
