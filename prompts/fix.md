A reviewer checked your change for issue #{{number}} on branch {{branch}}. The review is in {{review_path}}.

Fix every must-fix finding, and each should-fix finding that stays within the issue. Same rules as before: follow the
sdlc-implementation skill if the repository has one, work test first, commit after every green step, and run these
checks after each step:

{{gates_sandbox}}

Fix the kind of problem, not only the place the reviewer named. For each finding, name the kind of problem in one
sentence, search your change for every other place it occurs, and fix them together, or say why a place is left. A
finding marked "Repeat of:" means an earlier fix patched one instance and the kind came back: don't patch the next
instance. Change the structure so the kind can't occur, or, if that is outside this issue, leave the code and say
so with the reason.

A finding is no licence for a change that is the user's to decide:
- adding, removing or reinterpreting a recorded decision, or working around a rule one sets;
- adding a dependency, or code whose main purpose is to avoid one;
- adding something that needs upkeep outside the feature (generated code, a script, a second crate, a new file format
  or config key);
- adding user-visible behavior the issue doesn't ask for;
- a fix much larger than its finding.

When a finding can only be fixed by such a change, don't make it and don't build around it: leave the code as it is
and hand the question back.

Run the checks in the foreground and wait for them. This is a headless run: it ends when you end your turn and nothing
resumes it, so never end your turn while waiting on a background command.

Don't rewrite existing commits, push or change remotes. If you think a finding is wrong, don't change the code for it;
say why.

Then add a "Review" section to .sbxm-task/result.md: each finding as fixed (with the commit) or not (with the reason),
with the kind of problem and the other places you checked. Add to its "Choices made" section each choice a fix made
that someone could reasonably have made differently (one line, with the reason), and to its "Needs a decision"
section each question you hand back: the finding, the options, and the one you would pick.
