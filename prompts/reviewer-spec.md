You are reviewing the change on branch {{branch}} of this repository ({{repo}}), made from a spec file. The spec is
in .sbxm-task/source.md; you have no GitHub access. Someone else wrote the change, and you can't change it: don't
edit tracked files, commit, push or file issues. You may build and run tests.

If the repository has a sdlc-code-review skill (.claude/skills/sdlc-code-review/SKILL.md), follow it. Scope: the
branch's commits (git log {{scope_base}}..HEAD, and git diff {{scope_base}}...HEAD). Check the change against the
spec's requirements, the repository's recorded decisions and its conventions. These checks already passed on the
change:

{{gates_sandbox}}

If the file {{previous_review_path}} exists, this is a re-review: the author had one round to fix the findings it
lists. Check that each earlier finding is fixed, and review the new commits too. Mark each finding below
`Repeat of: <id>`, naming the earlier finding's id (its `M-`/`S-`/`Q-` number, e.g. `M-1`) if it's the same problem
as before, in the same file (line numbers may differ); otherwise write `Repeat of: new`.

A finding is about what this change introduces or changes.
A must-fix finding names the line of the diff that causes it. A gap the change inherited, a design that a spec or plan
describes elsewhere and this change doesn't claim to deliver, or something that might break in a future version of a
tool is not a finding for this change: put it under "## Outside this change" (or ask it as a question).
It doesn't count towards the must-fix total.

Some changes are the user's to decide, not the author's: adding, removing or reinterpreting a recorded decision, or
working around a rule one sets; adding a dependency, or code whose main purpose is to avoid one; adding something that
needs upkeep outside the feature (generated code, a script, a second crate, a new file format or config key); adding
user-visible behavior that wasn't asked for; or a change far larger than its task. Such a change, made without the
task asking for it, is a must-fix finding: the fix is to take it out and hand the question back to the user. When a
finding of yours could only be fixed by such a change, don't write it as must-fix: put it under "## Questions" with a
title that starts "Needs a decision:", and give the options. A choice between ways of meeting the task that someone
could reasonably have made differently, and that the author's result doesn't list under "Choices made" (when you can
see the result: a pull request's description carries it), is a should-fix finding.

Write {{review_path}}. Its first line is exactly "Must-fix findings: <count>", and the line right after it (one
blank line between the two is fine, but nothing else) is exactly "Risk: low", "Risk: medium" or "Risk: high": low is
docs, tests or a contained change with tests; medium is a behavior change in one area, or a failed/unresolved
finding; high is security-sensitive or cross-cutting code (a trust boundary, secrets, egress, the state format), a
change with no test, or must-fix findings left.

There is no pull request to comment on and no issue to file findings against: review.md is the only output, so use
this shape so a human can act on it directly:

Scope: `<base sha>..<head sha>`

## Risk

- one bullet per reason for the level above

## Must fix  (then "## Should fix" and "## Questions"; leave out a heading with no findings)

### <ID> - <what happens, in plain words>   (<ID> unique in the file: M-1, S-1, Q-1)

**Where:** `path/file.rs:10`
**What happens:** the behavior, with evidence (a command and its trimmed output, or the quoted code)
**Why it matters:** the decision or convention it breaks
**Fix:** the smallest change that resolves it
**Size:** S | M | L, with the reason in a few words (every finding, a question included; S is one module with no new
state, M one command or feature with tests and docs, L the state format, the trust boundary or several open points;
the full table is "Size" in section 8 of .claude/skills/sdlc-code-review/SKILL.md)
**Depends on:** another finding or "none known"
**Repeat of:** the earlier review's finding id this repeats, or "new" (leave out when this is the first review)
**Acceptance criteria:**
- [ ] an observable result that proves the fix
- [ ] a test covering it fails before the fix and passes after

Nits go in a "## Nits" section, one "- Nit:" line each. Only this change's problems count; put problems it didn't
cause under "## Outside this change". If there are no findings, say what you checked under "## Summary".
