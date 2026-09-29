# Review: slice 0 exec backend

Scope: `HEAD~1..1517f18` (`1517f18 Add exec and skills to SandboxBackend; make backends Send + Sync`).

## Findings

### Should fix — real `sbx` coverage does not exercise the new backend operations

**Where:** `src/backend/sbx.rs:79` (`exec`) and `src/backend/sbx.rs:111` (`skills`).

**What happens:** The new tests assert argument construction and `FakeBackend` behavior, but none exercises `SbxBackend::exec` or `SbxBackend::skills` against the `sbx` CLI. `tests/real_sbx.rs` has no `exec` or `skills` calls (its real-`sbx` tests cover lifecycle and harness instruction files only). Thus command invocation, stdin forwarding/EOF, captured output and exit status, and skills JSON retrieval have no integration coverage.

**Why it matters:** The code-review skill's section 5 requires an ignored real-`sbx` test when a change touches `sbx` interaction; the implementation skill likewise says the real backend for a slice gets an `#[ignore]` integration test. This leaves the central new backend boundary unchecked against the CLI it wraps.

**Fix:** Add an ignored integration test using a uniquely named sandbox from the existing real-`sbx` fixture/cleanup pattern. Exercise `exec` with closed stdin and piped stdin, assert stdout/stderr/exit code, and call `skills` to confirm a JSON value is returned. Run it only with the dedicated real-test base dir and required `sbx` setup.

**Depends on:** none known.

**Related:** none known.

**Acceptance criteria:**
- [ ] An ignored real-`sbx` test exercises `SbxBackend::exec` and `SbxBackend::skills`.
- [ ] `exec` coverage asserts working directory, piped EOF/input behavior, output, and exit status.
- [ ] The test cleans up its uniquely named sandbox even on failure.
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` pass.
- [ ] The ignored integration test passes with the dedicated real-`sbx` base dir and leaves no sandbox behind.
- [ ] No user-visible change.

**Review:** `reviews/2026-09-29-slice-0-exec-backend.md`.

Issues: filed as #32 (https://github.com/tutros/sbxm/issues/32); fixed in `2cef778`.

## Check results

- `cargo fmt --check`: passed.
- `cargo clippy --all-targets -- -D warnings`: passed.
- `cargo test`: 246 passed, 5 ignored.
