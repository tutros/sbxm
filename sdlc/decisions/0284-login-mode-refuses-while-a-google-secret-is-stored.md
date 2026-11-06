284. **Login mode refuses while a `google` secret is stored globally (2026-11-03, issue 384 review M-1, the user chose
option (a); refines [281](a)):** the pinned Antigravity kit declares one credential source and one proxy-managed
variable (`sbx kit inspect`), so `sbx` injects a stored global `google` secret as `GEMINI_API_KEY` into every
Antigravity sandbox, and a mixin from sbxm can't remove it. In `antigravity_auth = "login"`, `new`, `open` (create,
attach to an existing sandbox, and `--rebuild`) therefore refuse before anything is created or attached while
`google` is stored globally, naming `sbx secret rm google` and the `"api-key"` mode. Rejected for now: (b) a
sandbox-scoped dummy `google` secret to override the global one (`sbx secret set --sandbox`), unproven because no
real key was stored to test precedence (#384, comment of 2026-11-03), and (c) accepting the injection. Cost: login
mode and headless Antigravity (which needs `google` stored, [281](b)) can't be used at the same time; if both are
wanted, test (b) with a real key first. `open` runs the same check before attaching to an existing sandbox in either mode, so
an `"api-key"` sandbox whose `google` secret was removed is refused too, since `agy` would have no key (PR 408 narrow
review). **Interactive attach is checked by hand** (the user, 2026-11-03, PR 408 review M-2): no
automated test drives `sbxm open --harness antigravity`, because it needs a real terminal and a signed-in Google
account. Instead, on each change to Antigravity's interactive path, a person runs the four-step check (sign in on the
first `open`, a request answers, `stop` then `open` with no second sign-in, a request answers) and records the result
on the PR. The first such check passed on 2026-11-03 (#384, with `agy` 1.4.12 and the `mcp-gateway` entry present).
