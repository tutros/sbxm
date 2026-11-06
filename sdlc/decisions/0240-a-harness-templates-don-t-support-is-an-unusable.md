240. **A harness templates don't support is an unusable template (2026-10-22, PR 269 review round 2 M-1, the user
chose the recommendation; refines [204] with [200], [203]):** for a profile with `[template]`, creating an
`antigravity` or `gemini` sandbox is decided like any other unusable template, before `sbx` is asked anything: a
strict profile is refused with "templates are not supported for the <h> harness yet; use --harness claude, codex or
pi", and a non-strict one prints that as its one warning and uses the install steps. Neither suggests `template
build`, which refuses those harnesses.
