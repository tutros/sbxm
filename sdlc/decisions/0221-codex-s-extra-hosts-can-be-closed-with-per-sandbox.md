221. **`codex`'s extra hosts can be closed with per-sandbox deny rules, so strict `codex` sandboxes are possible
(2026-10-16, milestone 3 slice 0b, a real-`sbx` check on `sbx` 0.46.0; answers [218]; 220 is reserved for issue
258):** (a) The built-in `codex` kit's network rule (`sbx policy ls <sandbox> --json`: name `kit:<sandbox>`,
`provenance.created_via` `provisioned`, `editable: false`) allows the OpenAI hosts (`api.openai.com`, `openai.com`,
`auth.openai.com`, `chatgpt.com`, `files.openai.com`, `releases.openai.com`) plus `github.com`, `api.github.com`,
`codeload.github.com`, `release-assets.githubusercontent.com`, `registry.npmjs.org`, `download.docker.com` and
`archive`/`security`/`ports.ubuntu.com` (the last two hosts, npm and Docker, add to the list in [217](c)). It can't
be removed (`editable: false`), but (b) **`sbx create --deny-network HOST` (repeatable) wins over it:** with one
deny per non-OpenAI host, every one of the nine answered 403 (so did `example.com`), `api.openai.com` was still
reachable, and `codex exec` answered a canary from `~/.codex/AGENTS.md` (4.9 s, the whole command timed). The deny
rules are sandbox-scoped (`editable: true`), survive a stop and start, and are gone after `sbx rm`; the kit's startup
commands still finished (`apt-get update || true` is blocked, not fatal). (c) The same holds with `-t
docker.io/docker/sandbox-templates:codex-docker --pull never` (created in 5.0 s): the denied hosts answered 403,
while `api.github.com`, left off the deny list on purpose, answered 200. Not tried: a saved sbxm template with
sbxm's own kits (the image comes from `-t` and the network rule from the `codex` agent, so neither is expected to
change the result; end-to-end check 7 of `sdlc/milestone-3.md` covers it). **Verdict, for slice 6:** a strict `codex` sandbox is created
with one `--deny-network` per host the `codex` kit allows beyond the OpenAI hosts and the profile's
`[template].allow`, so [200](b) holds without refusing `codex`. The hosts can't be read before the sandbox exists, so the
deny list is a table in sbxm (the list in (a)). **Confirmed by the user 2026-10-16 (PR 265 review Q-1), for slice
6:** after creation and
before any agent command, sbxm reads the kit rule's hosts from `sbx policy ls <sandbox> --json` and, if one is
allowed that is neither an OpenAI host, in `[template].allow` nor denied, removes the sandbox and refuses, naming the
host, so a later `codex` kit that adds a host can't widen a strict sandbox silently.
