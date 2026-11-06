218. **Strict mode keeps its allow-list promise; `codex` gets strict templates only if its hosts can be restricted
(2026-10-16, PR 261 review Q-1, the user chose option (a)):** [200](b) stands: a strict sandbox gets only the
profile's runtime allow list. Because the built-in `codex` kit adds its own hosts [217](c), slice 0b of
`sdlc/milestone-3.md` first checks, with real `sbx` and bounded, whether `codex` can run with network rules limited
to `[template].allow` (for example a `codex` setup without the built-in kit's rules). If it can, sbxm uses that
setup for strict `codex` sandboxes. If it can't, a strict profile with the `codex` harness is refused with a message
naming [217](c), before any sandbox is created; non-strict `codex` templates stay supported. Rejected: amending [200]/[202] with
a documented `codex` floor of allowed hosts (option (b)), which would quietly weaken a confirmed security boundary.
The user noted that Antigravity may be similar (the right endpoints allowed might let it use the user's subscription
instead of an API key); that is a follow-up idea outside this milestone ([204]), not decided here.
