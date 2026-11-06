232. **The check after a strict `codex` creation fails closed (2026-10-19, milestone 3 slice 6 review Q-1, the user
agreed with the recommendation; adds to [221]):** sbxm checks every network allow rule `sbx policy ls` reports as
scoped to the sandbox (`scope: sandbox:<name>`), not only the kit's: each host must be an OpenAI host, in
`[template].allow` or denied by a rule. If no `kit:<sandbox>` allow rule is reported at all (a future `sbx` renaming
it, say), sbxm can't see what the kit allows, so it removes the sandbox and refuses too. A strict sandbox is kept
only when sbxm can see exactly what it allows; a change in `sbx` makes strict `codex` refuse until sbxm is updated.
Global rules (the user's own `sbx policy allow` for every sandbox) are left out: they are the user's deliberate
setting, outside any profile.
