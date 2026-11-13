231. **A strict profile is refused with the `pi` harness for now (2026-10-19, milestone 3 slice 6 review M3S6-1, the
user agreed with the recommendation; adds to [200], [218]):** Pi's own kit allows `registry.npmjs.org` besides its
provider (spike S15, question 4), so a strict `pi` sandbox would reach a host outside `[template].allow`. `new` (and
every later command that creates from a template) refuses a strict profile with `pi` before any write, naming the
host and the fix (`strict = false`, or `claude`/`codex`). Non-strict `pi` templates stay supported. Deferred: deny
the host as [221] does for `codex`, after a bounded real-`sbx` check that Pi still works without it (issue #266,
trigger: a strict profile wanted with `pi`). **Replaced by [323] (2026-11-13, issue #266):** a strict `pi` sandbox denies the Pi kit's extra hosts instead of
being refused.
