# Small review

Scope: `1111111111111111111111111111111111111111..2222222222222222222222222222222222222222` (2 commits)
Issues: pending (test)

## Must fix

### S-1 - Thing breaks

**Where:** `src/a.rs:10-20`, `README.md`
**What happens:** It breaks.
**Why it matters:** decision 7
**Fix:** Fix it.
**Depends on:** S-2 (the message must match)
**Acceptance criteria:**
- [ ] it works
- [ ] A test covering it fails before the fix and passes after (name it)
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Docs updated where behavior users see changed, or "no user-visible change"

### S-2 - Message is wrong

**Where:** `src/b.rs:5`
**What happens:** It says the wrong thing.
**Why it matters:** the error convention
**Fix:** Say the right thing.
**Acceptance criteria:**
- [ ] it says the right thing
- [ ] A test covering it fails before the fix and passes after (name it)
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Docs updated where behavior users see changed, or "no user-visible change"

## Questions

### S-Q1 - Keep the thing?

**Where:** `src/c.rs:1-3`
**What happens:** The thing is half built.
**Why it matters:** it blocks S-1.
**Recommendation:** Remove it; the other option is to finish it.
