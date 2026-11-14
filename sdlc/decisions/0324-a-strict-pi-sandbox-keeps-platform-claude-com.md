324. **A strict `pi` sandbox keeps `platform.claude.com`: Pi refreshes its OAuth token there (2026-11-14, issue #644,
the user chose option A, a code change rather than a profile allow entry; corrects [323](b)):** #638's real check (#641,
2026-11-14, `sbx` with binary 70ce06a) built a strict `pi` sandbox from a fresh `sbxm-dev` template: `registry.npmjs.org`
and `platform.claude.com` were both denied (403, as [323] meant), but `pi -p` failed with `OAuth refresh failed for
anthropic: ... url=https://platform.claude.com/v1/oauth/token; ... status=403; body=Blocked by local rule for
platform.claude.com:443`. [323](b) saw Pi answer without `platform.claude.com` only because its OAuth token was still
fresh; once the token needs a refresh, Pi needs the host, so denying it breaks every strict `pi` sandbox after a few
hours. **Verdict:** `platform.claude.com` is a host a strict `pi` sandbox keeps, like the provider host
`api.anthropic.com`: it is never denied, and the after-create check accepts the kit's allow rule for
`platform.claude.com:443` (compared without the port). A strict `pi` sandbox is created with `--deny-network
registry.npmjs.org` unless `[template].allow` names that host. A `[template].allow` entry naming `platform.claude.com`
(with or without the port) is harmless. Everything else in [323] stands.
