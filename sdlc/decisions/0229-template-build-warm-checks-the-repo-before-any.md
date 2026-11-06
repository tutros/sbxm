229. **`template build --warm` checks the repo before any write (2026-10-18, milestone 3 slice 5, made while the user
was away; confirmed by the user 2026-10-18):** a repo that is not a folder, has no `Cargo.toml` (until `[warm]` in slice 5b), has no
`Cargo.lock` (the `--locked` steps would fail in the sandbox anyway, and the record needs its hash) or holds a link
outside `.git` and `target` is refused before the first write. Links inside `.git` and the top `target` are neither
copied nor refused. The package names `cargo metadata` returns go into `cargo clean -p` only if they are made of
letters, digits, `-` and `_`. The warm-up steps use the default gate timeout (20 minutes each). Slice 5b: `[warm]`
needs both keys, a non-empty `commands` and `lock_files` (`[]` allowed, saying "never stale on dependencies"), each
lock file a relative path inside the repo that exists; `[warm]` replaces the cargo steps in a Rust repo too, and
`template build` reads only that table, so a repo that is not a task target can hold just `[warm]`.
