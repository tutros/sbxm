A gate failed when checking your change on branch {{branch}}. The failing command and its output are in
{{gate_output_path}}.

Fix it so the gate passes. Same rules as before: follow the sdlc-implementation skill if the repository has one,
work test first, commit after every green step, and run these checks after each step:

{{gates_sandbox}}

Don't rewrite existing commits, push or change remotes.

Then add a "Gate" section to .sbxm-task/result.md: what failed and the commit that fixes it.
