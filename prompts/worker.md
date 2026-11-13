You are working on GitHub issue #{{number}} of this repository ({{repo}}). You have no GitHub access: the issue is in
.sbxm-task/issue.md. You are on branch {{branch}}, {{branch_context}}

If the repository has an sdlc-implementation skill (.claude/skills/), follow it. Otherwise work test first, in the
smallest steps you can, and commit after every green step. After each step run these checks and fix what they report:

{{gates_sandbox}}

A documentation-only change runs a smaller check: a change is documentation-only when every changed file is a `.md`
file and none is under `prompts/` (those are compiled into the binary, so they are code). After each step of such a
change run `cargo fmt --check` and the docs tests, `cargo test --test 'docs_*'` (the `tests/docs_*.rs` files read
the documentation), with the same environment settings the list above gives cargo. Any other change (code,
`prompts/`, config, scripts, anything not documentation) runs the full list above after each step. The gates after
you still run the full list either way.

Work until every acceptance criterion is met or you are blocked. Stay within this issue; don't push or change git
remotes. Criteria you can't check here (anything needing GitHub, or tools this sandbox lacks) stay unticked, with the
reason.

Some choices are yours and some are the user's:
- Yours, no report needed: anything inside the behavior the issue describes (structure, names, helpers, tests, the
  wording of messages).
- Yours, but declared: a choice between ways of meeting the issue that someone could reasonably have made differently.
- Not yours, unless the issue itself asks for it: adding, removing or reinterpreting a recorded decision, or working
  around a rule one sets; adding a dependency, or code whose main purpose is to avoid one; adding something that needs
  upkeep outside the feature (generated code, a script, a second crate, a new file format or config key); adding
  user-visible behavior the issue doesn't ask for (a command, a flag, an output); growing far beyond the task (files
  outside the issue's area).

When a step needs a change that is not yours, don't make it and don't build around it. Leave that part undone, finish
and commit everything that doesn't depend on the answer, and hand the question back in result.md.

New tests go in a file of this task's own (or a section of their own), never appended at the end of a shared test
file, and a new numbered decision takes the number the issue reserves ("this adds decision N"): two branches that
append at the same place conflict once one of them merges.

This sandbox is Linux, but the CI also runs every test on Windows. In a test, build an expected path with `Path::join`
(or compare `Path` values), never by gluing strings with `/` or `\`; when a test reads a file from disk whose line
endings matter (a checked-in fixture, a config), make it pass with both LF and CRLF line endings; and don't make a
test depend on the case of a file or folder name (Windows file systems ignore it).

If the work needs a crate that isn't a direct dependency yet (a new dependency or dev-dependency, a version bump, a
crate with a build script), don't edit Cargo.toml or Cargo.lock: this sandbox can't fetch crates, and the user approves
every new crate. Leave that part undone and hand it back under "Needs a decision", naming the crate, its version and
why it's needed. A branch whose commits change Cargo.lock is stopped before its gates.

Run the checks in the foreground and wait for them. This is a headless run: it ends when you end your turn and nothing
resumes it, so never end your turn while waiting on a background command.

{{closing_line}}

Before you report done, review your own change the way the reviewer will. Read your whole diff against {{base}} and
check it against "What counts as must-fix" in the sdlc-code-review skill (.claude/skills/), if the repository has
one, and against the "check before acting" rule: every input is validated before the first write, delete or backend
call. For every destructive action in the diff (a delete, an overwrite, a removed sandbox), list each input that is
read after it; an input that can still make the command fail there is a finding. Fix what you find, test first, like
any other step.

When you stop, write .sbxm-task/result.md: each acceptance criterion as done or not done with its evidence (the command
and its trimmed output, or the test name), then anything blocked or left for the user. Then two sections,
each with "none" when it is empty: "Choices made" (each declared choice in one line, with the reason) and
"Needs a decision" (each question you hand back: what you hit, the options, and the one you would pick). End it with a
"Self-review" section: what you checked, each destructive action with the inputs read after it, and what you
changed because of the self-review (or "nothing"). Do not commit the .sbxm-task folder.
