194. **A free-space check of the drive holding `sbx`'s sandbox state (2026-10-13, decided with the user before issue
210 started, one point at a time, recorded on the issue; workflow gap G8, spike S11):** a sandbox's disk file never
shrinks, one test build of this repo is 5.2 GB, and a full drive makes `sbx` answer "try: sbx login". (a)
**Threshold:** 10 GiB free by default; one key in the global `config.toml`, `min_free_disk = "10g"` (`k`, `m`, `g`,
`t`, powers of 1024), changes it, and `"0"` switches the check off. (b) **Which drive:** on Windows the drive of
`%LOCALAPPDATA%\DockerSandboxes`, the only location verified (`sbx` v0.46.0 has no setting to move it and no command
that reports it); one optional key in the global `config.toml`, `sandbox_state_dir`, overrides it on any platform. On
macOS and Linux without the key the check is skipped: `doctor` says so in one line naming the key, the other
commands run, and sbxm guesses no path. (c) **Missing or unreadable:** if the folder doesn't exist, or its free
space can't be read, `doctor` warns and the other commands run. (d) **Which commands refuse** (the builder's reading
of "a command that would create a sandbox", not a separate answer of the user's): every command that can create
one, with its other refusals and before any write: `new`, `open` when it creates or rebuilds, `run`, and `task
start`, `task review`, `task run` and `task resume`; `doctor` only warns. The refusal names the drive, the free
space and the flag. `task run` checks again before the reviewer's sandbox, since the worker can fill the drive, and
that refusal names `task review`, which continues the task. (e) **Override:** `--ignore-disk` lets one command
through. Free space is read through `doctor`'s `Host` and `disk::Guard`, so no test depends on the machine.
