211. **Every template build gets a fresh tag (2026-10-15, PR 253 review Q-1, the user agreed with the
recommendation; changes the tag format of [209]):** the tag is `sbxm-<profile>-<harness>:<hash12>-<UTC build time>`
(for example `sbxm-sbxm-dev-claude:2a9974ee6210-20261015t2210`), so a save never lands on an existing tag. Why: the
lock hash is not in the tag [209], so a warm rebuild after a lock file change would reuse the tag, and `sbx template
save` onto an existing tag replaces the image silently (S11, question 8), which would break [205]'s promise to keep a
template still in use. Refusing the rebuild while the tag is in use was rejected: any open task would block it, and
strict sandboxes would be refused meanwhile. [205]'s cleanup is unchanged; staleness still compares the hash part,
which the record holds too.
