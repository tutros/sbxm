195. **`task rm --merged` cleans up everything sbxm made for merged tasks in one command (2026-10-13, decided with
the user, one point at a time, recorded on issue 202):** (a) **When a task counts as merged:** an issue task when
its PR is merged on GitHub; a PR-review task (`pr-N`) when PR N is merged (open, or closed without merging, is left
alone); a spec task when the last commit of its result branch is contained in the base branch of the repo it came
from, as `origin/<base>` stands after a fetch the command runs first. A branch merged by squash or rebase isn't
detected: that task is left alone and listed as "not merged", naming the plain `task rm --spec FILE` as the way to
remove it by hand. (b) **An archive before removal:** before removing a task, `task rm --merged` copies its text
records to `<base>/.sbxm/task-archive/<task-id>/` — `review.md`, every `review-<n>.md`, `result.md`, `run.log`,
`gates.log` and `task.json`, whichever exist. Never copied: transcripts, clones, `repo.git`. The summary names the
archive folder; nothing deletes or overwrites an archive (PR 241 review: a rerun after a failed removal reuses an archive holding exactly the same records, and one that differs is refused, naming the folder), and a plain `task rm` is unchanged and archives nothing. (c) **What it
never touches:** `--force` is refused together with `--merged`; a task whose result exists only in its own
`repo.git` and was never fetched [178] is left alone and listed with the reason, even if it would otherwise count
as merged. It removes only what sbxm made — sandboxes, clones and task folders; for a spec task with the `local`
sink it does not delete the worktree or the local branch in the user's own checkout, and the summary prints the two
`git` commands for each instead of running them. Without a terminal it needs `--yes`, like `task rm` today;
`--dry-run` lists and removes nothing.
