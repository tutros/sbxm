209. **The template hash and tag (2026-10-15, plan choice P3, the user agreed with the recommendation):** the
template hash covers the profile's `setup.install`, `network.allow` and `network.deny` (the build allow list), the
harness with its pinned `agent_kit` [104], and whether the template is warm; this is the hash of [203](a). The lock
file hash and the warm commands [207] are in the record, not the tag: the lock hash is checked at create time
[203](b); the warm commands are recorded but not checked, since a create cannot see the repo's `[warm]`, so changed
warm commands need a manual `template build --warm`. Tag: `sbxm-<profile>-<harness>:<first 12 hex of the hash>`
(profile names are already `[a-z0-9-]`, valid in an image name).
