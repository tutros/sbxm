Your change for {{branch}} waited before its review while other work touched the same files (decision 331). That work
has merged or ended, so your branch now catches up with {{base}} before it is reviewed. You have no GitHub access; stay
on branch {{branch}} and build on its commits.

Merge the base: first run `git fetch .sbxm-task/base.bundle +refs/heads/{{base}}:refs/remotes/origin/{{base}}` (the
bundle holds {{base}} as it is now), then `git merge --no-edit origin/{{base}}`. If it merges cleanly, you are done
with the merge. If it conflicts, resolve every conflict mechanically, keeping both sides whole: two sides that appended
at the same place (tests at the end of a file, decisions in a list) both stay, one after the other; a line both sides
edited (a README row) gets both edits, word by word. Then `git add` the files and `git commit --no-edit`. Never
rebase, reset or force. If a conflict needs a judgement about behavior, not only about text, run `git merge --abort`
and hand it back under "Needs a decision" in .sbxm-task/result.md, naming the files; the task then stops for the user.

After the merge, run these checks and fix what the merge broke, committing each fix:

{{gates_sandbox}}

Change nothing else: no new behavior, no refactoring. Run the checks in the foreground and wait for them. This is a
headless run: it ends when you end your turn and nothing resumes it, so never end your turn while waiting on a
background command.

Don't push or change remotes. Then add a "Merge" section to .sbxm-task/result.md: whether the merge conflicted, each
file you resolved and how, and the commits after it.
