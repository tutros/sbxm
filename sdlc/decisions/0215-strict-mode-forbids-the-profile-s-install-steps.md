215. **Strict mode forbids the profile's install steps, not sbxm's own configuration step (2026-10-15, PR 253
review round 2 Q-5, the user agreed with the recommendation; narrows [200](a)):** a strict sandbox runs none of the
profile's `setup.install` steps (software installs), but the harness kit's root step that writes Claude's
`managed_settings` [64] (`src/kit.rs`) still runs at every create. It writes one local file and needs no egress, so
a settings change applies without a rebuild; managed settings stay out of the template hash [209]. Baking them in
was rejected: every settings change would need a template rebuild.
