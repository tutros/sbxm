281. **Antigravity's Google login: in-sandbox for interactive, API key for headless (2026-11-02, the user's
choice; supersedes [270], closes the question of #330 and #340).** A hands-on check for #340 (`agy` 1.4.12,
2026-11-02) showed `agy` refreshes its own access token *inside* the sandbox over `oauth2.googleapis.com`, with no
host helper and no secret sbxm supplies (the client secret is compiled into `agy`). Route A ([270], a host-refreshed placeholder token)
is abandoned: it needs `agy`'s confidential client secret, which the binary no longer exposes to a working external
refresh (every id/secret pair answers `401 invalid_client` / `client_secret is missing`; `agy` 1.4.12), so it would
mean reverse-engineering and would break on `agy` updates ([270]'s own Q4 warned of this). Instead:
   - **(a) Interactive sandboxes (`sbxm new`/`open`):** the user signs in *inside* the sandbox once (`sbx run`,
     Google OAuth); `agy` holds the real token and self-refreshes. sbxm touches no token and stores no secret. The
     sandbox persists across `open`, so the login persists; a fresh `new` needs a fresh sign-in. This is issue #384
     (the login flow, dropping the `google`-secret requirement in this mode, docs, a real-`sbx` test).
   - **(b) Headless sandboxes (`sbxm run` contestants and judge, `sbxm task` workers and reviewers):** keep the
     `google` API key ([137]). They are throwaway and unattended, so a per-sandbox interactive login isn't possible,
     and injecting the user's real refresh token (≈9 scopes, incl. Drive) into an untrusted sandbox is the exposure
     [40][80] exist to prevent. The API key's prepaid cost is accepted for headless.
   - **(c) Not done:** PR #339 (route A's implementation) is closed. Its reusable parts — the `ureq` refresh
     transport ([271]) and reading `agy` 1.4.12's nested token file — are salvaged to issue #383 (branch `issue-330`
     kept) for if Antigravity auth is revisited.
