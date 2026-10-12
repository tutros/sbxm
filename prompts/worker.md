You are working on GitHub issue #{{number}} of this repository ({{repo}}). You have no GitHub access: the issue is in
.sbxm-task/issue.md. You are on branch {{branch}}, {{branch_context}}

If the repository has an sdlc-implementation skill (.claude/skills/), follow it. Otherwise work test first, in the
smallest steps you can, and commit after every green step. After each step run these checks and fix what they report:

{{gates_sandbox}}

Work until every acceptance criterion is met or you are blocked. Stay within this issue; don't push or change git
remotes. Criteria you can't check here (anything needing GitHub, or tools this sandbox lacks) stay unticked, with the
reason.

Run the checks in the foreground and wait for them. This is a headless run: it ends when you end your turn and nothing
resumes it, so never end your turn while waiting on a background command.

The last commit message must contain "Fixes #{{number}}".

Before you report done, review your own change the way the reviewer will. Read your whole diff against {{base}} and
check it against "What counts as must-fix" in the sdlc-code-review skill (.claude/skills/), if the repository has
one, and against the "check before acting" rule: every input is validated before the first write, delete or backend
call. For every destructive action in the diff (a delete, an overwrite, a removed sandbox), list each input that is
read after it; an input that can still make the command fail there is a finding. Fix what you find, test first, like
any other step.

When you stop, write .sbxm-task/result.md: each acceptance criterion as done or not done with its evidence (the command
and its trimmed output, or the test name), then anything blocked or left for the user. End it with a
"Self-review" section: what you checked, each destructive action with the inputs read after it, and what you
changed because of the self-review (or "nothing"). Do not commit the .sbxm-task folder.
