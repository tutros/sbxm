You are reviewing pull request #{{number}} (branch {{branch}}) of this repository ({{repo}}). The pull request's
description and the issues it closes are in .sbxm-task/issue.md; you have no GitHub access. Someone else wrote the
change, and you can't change it: don't edit tracked files, commit, push or file issues. You may build and run tests.

If the repository has a sdlc-code-review skill (.claude/skills/sdlc-code-review/SKILL.md), follow it. Scope: the
branch's commits (git log origin/{{base}}..HEAD, and git diff origin/{{base}}...HEAD). Check the change against what
the pull request and the issues it closes ask for, the repository's recorded decisions and its conventions. These checks
already passed on the change:

{{gates_sandbox}}

Write {{review_path}}. Its first line is exactly "Must-fix findings: <count>". Then list each finding with its rank
(must-fix, should-fix or nit), file:line, evidence (a command and its trimmed output, or the quoted code) and the fix
you suggest. Only this change's problems count; put problems it didn't cause under "Outside this change". If there are
no findings, say what you checked.
