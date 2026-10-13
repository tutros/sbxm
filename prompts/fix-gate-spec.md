A gate failed when checking your change on branch {{branch}}. The failing command and its output are in
{{gate_output_path}}.

Fix it so the gate passes. Same rules as before: follow the sdlc-implementation skill if the repository has one,
work test first, commit after every green step, and run these checks after each step:

{{gates_sandbox}}

A failing gate is no licence for a change that is the user's to decide:
- adding, removing or reinterpreting a recorded decision, or working around a rule one sets;
- adding a dependency, or code whose main purpose is to avoid one;
- adding something that needs upkeep outside the feature (generated code, a script, a second crate, a new file format
  or config key);
- adding user-visible behavior the spec doesn't ask for;
- a fix much larger than the failure.

When the gate can only pass with such a change, don't make it and don't build around it: leave the code as it is and
hand the question back.

Run the checks in the foreground and wait for them. This is a headless run: it ends when you end your turn and nothing
resumes it, so never end your turn while waiting on a background command.

Don't rewrite existing commits, push or change remotes.

Then add a "Gate" section to .sbxm-task/result.md: what failed and the commit that fixes it. Add to its
"Choices made" section each choice the fix made that someone could reasonably have made differently (one line, with
the reason), and to its "Needs a decision" section each question you hand back: the failure, the options, and the one
you would pick.
