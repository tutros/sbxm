190. **`task review --pr N` reviews a `ready` PR task again in place instead of refusing (2026-10-12, decided with
the user before issue #195 started, recorded on the issue itself; gap G34):** a second review round used to need two
hand steps (copy `review.md` aside, `task rm --pr N`) every time a PR's author pushed a fix, and forgetting the
copy lost the earlier round's review — the PR comment on GitHub was the only other copy. Now: (a) **An unchanged
head is refused unless `--force`.** When the PR's current head (from `gh pr view`, fetched into `repo.git`) is
still the commit the task's last review covered (`review.commit`, now also set for a PR task's own review, which it
never was before), the command refuses before writing anything, naming that commit and the `--force` command; this
is the same `--force` flag `task review --pr` already has for decision [187]'s refusal, not a second flag. (b)
**G38, the reviewer's half only:** from the second round on, the reviewer gets every earlier round's saved
`review-<n>.md` as context (`previous-review.md`, the same mechanism issue 169 built for a re-reviewed issue task),
and `review.md` opens with a "Status of earlier findings" section the reviewer writes from them: every earlier
finding (must-fix, should-fix or question) marked fixed (naming the commit), still open, or deferred to a GitHub
issue, judged from the current commits rather than the pull request's own claims. Recording the status of a hand
fix made outside a review round, and editing earlier PR comments, are not part of this: G38 stays open in
`sdlc/evals-workflow-notes.md` for that half. (c) **One new state machine row:** `ready` with status `ok`, PR tasks only, plus the `task review` event
(and the `Record::advance` move it drives) goes to `gating`: fetch the PR's new head, keep the last review's
`review-<n>.md`, run the gates (they run again, since the commits are new), then the reviewer, numbering the round
after every saved `review-<n>.md` the same way `next_review_round` already does for an issue task. A forced review
of an unchanged head takes the same row; there is no second path that skips the gates. (d) **Nothing is written
before the refusals and the fetch:** a refused attempt, or one whose fetch of the PR's head fails, leaves the task
`ready` with its `review.md` and `task.json` as they were. A PR task still has no fix round and never repeats a
finding (no `Repeat of:` field in its prompt): the no-progress rule stays issue/spec-only, since only they have a fix
round of their own to make no progress in.
