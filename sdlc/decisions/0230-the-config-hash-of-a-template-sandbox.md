230. **The config hash of a template sandbox (2026-10-18, milestone 3 slice 6, made while building, confirmed by the user 2026-10-18; adds
to [208]):** a profile with `[template]` adds a `template` part to the config hash: `strict` in both variants (it
decides whether an install sandbox may exist at all, so turning it on is drift for one), and `[template].allow` only
in a template sandbox's hash (its kit's runtime allow list; an install sandbox never sees it). A profile without
`[template]` hashes as before, so existing sandboxes show no drift. The two variants of a profile therefore have
different hashes and their kits sit in different folders. The template's tag is not hashed: a template rebuilt
later is never drift [208].
