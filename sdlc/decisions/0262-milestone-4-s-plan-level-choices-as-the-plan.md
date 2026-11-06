262. **Milestone 4's plan-level choices, as the plan recommended (2026-10-24, the user's answers to milestone 4's
P-choices):** the user confirmed each of these as `sdlc/milestone-4.md` recommends it.
(a) P1: `location` takes `"local"` or `"cloud"`, default `"local"`, and any other value is an error naming the file.
(b) P2: a short capture run by hand before slice 1 saves the cloud's real output as fixtures and tries the points S14
left open.
(c) P3: `SbxBackend` gains a location, and a cloud task gets the instance that prefixes every call with `--cloud`.
(d) P4: a cloud task's clone lives at `$HOME/work/<task id>`, the path read once with `exec printenv HOME` and recorded
in `task.json`.
(e) P6: a removal that fails with a message naming a retry is retried after 5 s, 10 s and 20 s, through an injected
sleep; any other error is reported at once.
(f) P8: a cloud task whose profile names a secret other than a provider OAuth login is refused, naming the secret and
`--local`, and a profile naming the other provider's login is checked at global scope like the harness's own ((a)
and (d)).
(g) P10: a non-strict `[template]` profile in the cloud is built from its kits, with one line saying cloud templates
aren't used.
(h) P11: the free-disk check is skipped for a cloud create, `--ignore-disk` still accepted, which narrows [194]'s
scope for a cloud create.
(i) P12: a cloud command needs `sbx` 0.46.0 or later, a constant like [213]'s, checked only for a cloud command.
(j) P13: `doctor` checks the current directory's repo when its `sbxm-task.toml` says `location = "cloud"`, and warns for
a gate command containing `/tmp/` or ending in `/tmp` as a word ((a) with (i)).
(k) P14: only `task start`, `task run` and `task review --pr N` take `--cloud`/`--local`; every other command uses
the location recorded in `task.json`.
(l) P15: `task start --workers N` is unchanged in the cloud.
(m) P16: the user edits their own `sbxm-task.toml` to build under `$HOME`, and the docs show the line; no
`sbxm-task.toml` is added to the repo.
(n) P17: when the cloud can't be read, `task status` reads only `task.json` and shows the recorded location,
`status` prints one line in place of its cloud rows and exits 0, and `doctor` shows a warning, not a failed check.
