200. **Strict mode (2026-10-15, the user agreed with the recommendation):** strict is a profile setting. A sandbox
of a strict profile (a) runs no install steps: everything comes from the template; (b) gets only the profile's
runtime allow list (empty for this repo), while the build allow list is used only to build the template; (c) is
refused when its template is missing, never quietly created with installs instead. Out of scope: the Claude kit's own
`apt-get update` at startup (blocked by egress, the sandbox starts anyway, S11) and forcing cargo's offline mode (a
profile can set `CARGO_NET_OFFLINE` in its own env). Why: the user does not want workers installing unsanctioned
software or profiles' host lists creeping (2026-10-02); a quiet fallback would undo both.
