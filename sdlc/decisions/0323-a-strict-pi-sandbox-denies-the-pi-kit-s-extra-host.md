323. **A strict `pi` sandbox denies the Pi kit's extra hosts, so a strict profile works with `pi` (2026-11-13, issue
#266, a real-`sbx` check on `sbx` 0.46.0; replaces [231]'s refusal; extends [221] and [232] to `pi`):** (a) The
built-in `pi` kit's network rule (`kit:<sandbox>` in `sbx policy ls <sandbox> --json`) allows three hosts, not the two
spike S15 suggested: `api.anthropic.com` (Pi's provider host, provider secret `anthropic`), `registry.npmjs.org` and
`platform.claude.com:443`. (b) With `sbx create --deny-network registry.npmjs.org --deny-network
platform.claude.com:443`, Pi answered headless and followed a canary in `~/.pi/agent/AGENTS.md`, and both denied hosts
answered 403 (Pi works on the `anthropic` OAuth secret without `platform.claude.com`); the captured rules are
`src/backend/fixtures/sbx-policy-ls-pi.json`. **Verdict:** a strict `pi` sandbox from a template is created with one
`--deny-network` per non-provider host of the Pi kit whose host (without the port) `[template].allow` doesn't name,
written as the kit writes it (`platform.claude.com:443` keeps its port), and is then checked as [221] and [232] check
a strict `codex` sandbox, with `api.anthropic.com` as the provider host: a scoped allow rule's host that is neither
the provider host, in `[template].allow` nor denied removes the sandbox and refuses (fails closed). An allow entry
names a host: `platform.claude.com` keeps the host, while `platform.claude.com:443` matches no host, so the host is
still denied. This replaces [231]: a strict profile is no longer refused with `pi`. A strict `pi` profile in the
cloud stays refused ([264]: sbxm keeps no cloud provider list for `pi`).
