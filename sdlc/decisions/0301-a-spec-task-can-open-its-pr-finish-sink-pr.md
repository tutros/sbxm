301. **A spec task can open its PR: `[finish] sink = "pr"` (2026-11-05, the user's answers on issue #438; uses
[174], [187], [244], [286]):** since [286] groups issues into one spec task, every group's PR was made by hand (a
fetch, a push, `gh pr create` with a hand-written body, then a second full review the in-task review already covered;
seen three times, PRs #426, #437, #446). (1) **A third value of the key that exists.** `[finish] sink = "pr"` in
`sbxm-task.toml`, no new flag; this repo sets it. Under it, `task start --spec` and `task run --spec` refuse before
any sandbox when the repo has no GitHub `origin` `gh` can reach (the repo `--repo` or the checkout's `origin` names,
asked for its default branch). (2) **`Fixes:` is one explicit line in the spec.** A plain line `Fixes: #435, #436`
anywhere in the spec (not front matter); numbers the spec only mentions are never used. At the start every number
must be an open issue of the repo (not closed, not a PR, not missing), or the start refuses naming it; a line that
starts with the word "Fixes" and then `:`, `#` or a digit but isn't exactly that form is refused naming the line and
the form, and two such lines as ambiguous. The same number twice is one. No line is allowed: the start warns, and the
PR body says it closes no issue. The finish reads the line from the copy the start saved (`source.md`), before
anything is pushed. (3) **The in-task review is the PR's review ([187]), as for an issue task.** `task finish --spec`
pushes the branch, opens the PR (a draft when must-fix findings are left or the task stopped, as #120; an open PR of
the branch into the base is recorded instead), with the risk first and one `Fixes #n` line per number in the line's
order, and posts the in-task review as its review comment with the `sbxm/review` status, the PR's head being the
reviewed commit; `task review --pr N` treats that head as reviewed, as it does an issue task's. The PR's title is the
spec's first `# ` heading, else the file name. No hand Windows test: CI's `windows-latest` job runs every test.
**Out of scope:** `task loop` taking spec tasks (its queue is labelled issues); trigger: the first backlog group to
run unattended.
