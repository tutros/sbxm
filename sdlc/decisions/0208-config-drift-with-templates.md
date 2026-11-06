208. **Config drift with templates (2026-10-15, plan choice P2, the user agreed with the recommendation):** a sandbox
records the template it was created from, or none [206]. `open`'s drift check [55] recomputes the hash of the same
variant: the baked kit for a sandbox from a template, the install kit otherwise. So a template built or removed later
is never drift. When a non-strict sandbox was created with installs and a matching template now exists, `open`
prints one hint naming `open --rebuild`; it does not refuse.
