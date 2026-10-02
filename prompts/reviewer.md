You are reviewing the change on branch {{branch}} of this repository ({{repo}}), made for GitHub issue #{{number}}.
The issue is in .sbxm-task/issue.md; you have no GitHub access. Someone else wrote the change, and you can't change it:
don't edit tracked files, commit, push or file issues. You may build and run tests.

If the repository has a sdlc-code-review skill (.claude/skills/sdlc-code-review/SKILL.md), follow it. Scope: the
branch's commits (git log origin/{{base}}..HEAD, and git diff origin/{{base}}...HEAD). Check the change against the
acceptance criteria of the issue, the repository's recorded decisions and its conventions. These checks already passed
on the change:

{{gates_sandbox}}

If the file {{previous_review_path}} exists, this is a re-review: the author had one round to fix the findings it
lists. Check that each earlier finding is fixed, and review the new commits too.

Write {{review_path}}. Its first line is exactly "Must-fix findings: <count>". Then, so the findings can be filed as
GitHub issues later, use this shape:

Scope: `<base sha>..<head sha>`
Issues: pending

## Must fix  (then "## Should fix" and "## Questions"; leave out a heading with no findings)

### <ID> - <what happens, in plain words>   (<ID> unique in the file: M-1, S-1, Q-1)

**Where:** `path/file.rs:10`
**What happens:** the behavior, with evidence (a command and its trimmed output, or the quoted code)
**Why it matters:** the decision or convention it breaks
**Fix:** the smallest change that resolves it
**Depends on:** another finding or "none known"
**Acceptance criteria:**
- [ ] an observable result that proves the fix
- [ ] a test covering it fails before the fix and passes after

Nits go in a "## Nits" section, one "- Nit:" line each. Only this change's problems count; put problems it didn't
cause under "## Outside this change". If there are no findings, say what you checked under "## Summary".
