**Where:** [`src/b.rs:5`](https://github.com/o/r/blob/2222222222222222222222222222222222222222/src/b.rs#L5)
**What happens:** It says the wrong thing.
**Why it matters:** the error convention
**Fix:** Say the right thing.
**Depends on:** none known
**Related:** S-1 depends on this one
**Acceptance criteria:**
- [ ] it says the right thing
- [ ] A test covering it fails before the fix and passes after (name it)
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Docs updated where behavior users see changed, or "no user-visible change"
**Review:** `sdlc/reviews/review-small.md`, finding S-2, reviewed commits `1111111111111111111111111111111111111111..2222222222222222222222222222222222222222`
<!-- review-finding: review-small.md#S-2 -->
