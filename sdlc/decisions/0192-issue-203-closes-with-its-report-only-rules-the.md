192. **Issue 203 closes with its report-only rules; the sandbox-starting commands move to issue 201 (2026-10-13,
both parts agreed by the user in the session that day):** (a) **Report-only `sbxm` commands** (`status`, `task states`, `task logs`, `list`, `config show`, `run show`, and `config profiles-dir`, added by PR 236's review, S-1) may run without an
approval prompt, but per machine: through a wrapper script at a fixed path, with the rules in
`.claude/settings.local.json`, because a rule in the checked-in `.claude/settings.json` cannot name a binary whose
file name changes with every build. The user adds the rules; an agent session cannot edit its own permissions.
(b) **`doctor` and `task status` stay on approval** (PR 236's review, M-1 and M-3): `doctor`'s writability check
creates and removes a file in the base dir and its kit check writes and removes kits in the temp dir, and `task
status` appends its output to the `run.log` of every task it shows [170], so neither only reads. The user had
agreed to a list with both in it; the review took them out, and `sbxm status` covers what `task status` was for.
(c) **Issue 203's item 2 is deferred, not answered:** whether `sbxm task start`, `review`, `run`, `resume` and
`gates` run without a prompt is decided with issue 201's other open points, where one command over a list makes the
question concrete; until then they stay on approval, and the user sees the exact rules before any are added.
