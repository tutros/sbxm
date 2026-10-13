You are working from a spec file in this repository ({{repo}}). It is in .sbxm-task/source.md; you have no GitHub
access. You are on branch {{branch}}, which started from {{base}}.

If the repository has an sdlc-implementation skill (.claude/skills/), follow it. Otherwise work test first, in the
smallest steps you can, and commit after every green step. After each step run these checks and fix what they report:

{{gates_sandbox}}

Work until the spec is fully implemented or you are blocked. Stay within the spec; don't push or change git remotes.
Anything you can't check here (anything needing GitHub, or tools this sandbox lacks) stays unticked, with the reason.

Some choices are yours and some are the user's:
- Yours, no report needed: anything inside the behavior the spec describes (structure, names, helpers, tests, the
  wording of messages).
- Yours, but declared: a choice between ways of meeting the spec that someone could reasonably have made differently.
- Not yours, unless the spec itself asks for it: adding, removing or reinterpreting a recorded decision, or working
  around a rule one sets; adding a dependency, or code whose main purpose is to avoid one; adding something that needs
  upkeep outside the feature (generated code, a script, a second crate, a new file format or config key); adding
  user-visible behavior the spec doesn't ask for (a command, a flag, an output); growing far beyond the task (files
  outside the spec's area).

When a step needs a change that is not yours, don't make it and don't build around it. Leave that part undone, finish
and commit everything that doesn't depend on the answer, and hand the question back in result.md.

Run the checks in the foreground and wait for them. This is a headless run: it ends when you end your turn and nothing
resumes it, so never end your turn while waiting on a background command.

Before you report done, review your own change the way the reviewer will. Read your whole diff against {{base}} and
check it against "What counts as must-fix" in the sdlc-code-review skill (.claude/skills/), if the repository has
one, and against the "check before acting" rule: every input is validated before the first write, delete or backend
call. For every destructive action in the diff (a delete, an overwrite, a removed sandbox), list each input that is
read after it; an input that can still make the command fail there is a finding. Fix what you find, test first, like
any other step.

When you stop, write .sbxm-task/result.md: what you did, with its evidence (the command and its trimmed output, or
the test name), then anything blocked or left for the user. Then two sections, each with "none" when it is empty:
"Choices made" (each declared choice in one line, with the reason) and "Needs a decision" (each question you hand
back: what you hit, the options, and the one you would pick). End it with a "Self-review" section: what you checked,
each destructive action with the inputs read after it, and what you changed because of the self-review (or
"nothing"). Do not commit the .sbxm-task folder.
