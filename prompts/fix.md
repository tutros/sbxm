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

Don't rewrite existing commits, push or change remotes. If you think a finding is wrong, don't change the code for it;
say why.

Then add a "Review" section to .sbxm-task/result.md: each finding as fixed (with the commit) or not (with the reason),
with the kind of problem and the other places you checked.
