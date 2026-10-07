A reviewer checked your change on branch {{branch}}. The review is in {{review_path}}.

Fix every must-fix finding, and each should-fix finding that stays within the spec. Same rules as before: follow the
sdlc-implementation skill if the repository has one, work test first, commit after every green step, and run these
checks after each step:

{{gates_sandbox}}

Don't rewrite existing commits, push or change remotes. If you think a finding is wrong, don't change the code for it;
say why.

Then add a "Review" section to .sbxm-task/result.md: each finding as fixed (with the commit) or not (with the reason).
