210. **Where a template is built (2026-10-15, plan choice P4, the user agreed with the recommendation):** the build
sandbox is `sbxm-template-<profile>-<harness>`, its workspace `<base>/.sbxm/templates/build/<profile>-<harness>/`
(never on `C:`, [56]). Both are removed after the build whether it passes or fails, and a `template build` after an
interrupted one removes their leftovers first. No agent ever runs in a build sandbox: its home folder goes into the
image, so agent sessions and transcripts would too (S11, risk 6).
