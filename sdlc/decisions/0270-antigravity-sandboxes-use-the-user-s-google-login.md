270. **Antigravity sandboxes use the user's Google login through a proxy placeholder (2026-10-29, the user's choice
after spike S16; the user asked for it because Codex limits are being hit; builds on [7], [137]):** route A of #288
is adopted. A sandbox holds only a token file with placeholder values and a far-future `expiry`; a custom secret
scoped to the sandbox, on `daily-cloudcode-pa.googleapis.com` and `www.googleapis.com`, has `sbx`'s proxy swap in an
access token that a host helper refreshes with `agy`'s own OAuth client, so the refresh token never enters a sandbox.
The helper is called for every proxied request, so it caches the token and must start fast; the MCP gateway entry
(#287) is emptied before every run; a `real_sbx` check guards against `agy`'s self-updates. The API-key route [137]
stays as the fallback. The build is issue #330 (size L, five open points to answer before a worker starts).
Write-up: `sdlc/spikes/S16.md`.
