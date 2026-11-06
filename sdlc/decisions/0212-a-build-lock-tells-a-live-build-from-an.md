212. **A build lock tells a live build from an interrupted one (2026-10-15, PR 253 review Q-2, the user agreed with
the recommendation; adds to [210]):** `template build` writes `<base>/.sbxm/templates/<profile>-<harness>.build.json`
with its process id and start time, the identity check of a task's `interrupted` test [163], reusing that helper. A
second build while that process is alive is refused, naming the process; when the process is gone, the leftovers of
[210] are removed and the build goes on. The lock file is removed at the end whether the build passes or fails. Why:
[210]'s fixed names plus its leftover cleanup would let a second build remove a first build's live sandbox. Unique
names per build (leftovers hard to find) and refusing whenever leftovers exist (no automatic recovery) were rejected.
