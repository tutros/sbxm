201. **A template is built only on the user's command (2026-10-15, the user agreed with the recommendation):** only
`sbxm template build` builds one; sbxm never builds or rebuilds a template on its own. A missing or stale template
gives the refusal of [200] (strict) or today's installs (not strict), and the message names the exact `template
build` command. The build goes through the free-disk check of [194] like every command that creates a sandbox, and
there is no rebuild schedule. Why: a build takes minutes, opens the wide build allow list and adds about 2.6 GB on
the sandbox drive, so it stays a deliberate step.
