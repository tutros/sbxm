# Evals milestone: workflow notes (started 2026-10-02)

Working through the deferred evaluators (cosine, spike S9, then Jev) with `sbxm task` wherever possible. This file
records, as it happens, what sbxm could not do and which steps a small script (a `just` recipe or a `sbxm` command)
could define. Each entry says what happened, the workaround used, and the candidate fix. Nothing here is decided.

## Missing functionality in sbxm

| # | Gap | Evidence | Workaround used | Candidate |
|---|---|---|---|---|
| G1 | **A task can't get extra egress.** Building `fastembed` needs `cdn.pyke.io` (ONNX Runtime, sha256 pinned in `ort-sys`'s `dist.tsv`); the gates compile every crate, so a worker or a gate run in a sandbox without it fails. Profiles are shared and deny-by-default. | cosine issue #63 | Ask the user to allow the host in `sbxm-dev` | `[sandbox] extra_allow = ["host", ...]` in `sbxm-task.toml` (reviewed like any config, recorded in the kit hash), or a per-issue profile via `--profile` |
| G2 | **Default Rust gates fail on a Windows host** whenever the workspace is a host mount: linking a test binary into it gave `Permission denied`. | PR 57 review runs 1-2 | Local `sbxm-task.toml` with `CARGO_TARGET_DIR=/tmp/target` on every cargo gate | `task init` and the built-in Rust default should put the target dir in the sandbox (`/tmp/target`) |
| G3 | **A gate's output is cut to its last 8 KB**, so a failing test's panic text is hidden behind the "Running ..." lines. | PR 57 round 2 gate log | A gate command that greps failures to the end | Keep the full output in a file next to `gates.log` and put failure sections in the tail |
| G4 | **`sbxm task` blocks**; no detach, wait or progress command. | every review run | `Start-Process` with log files, then polling | `--detach` plus `task wait`, or `task logs --follow` |
| G5 | **The local review config is not part of the repo** (`sbxm-task.toml` with the machine's gates and the E2E config dir), so each run is: build, copy binary and config in, set `SBXM_CONFIG_DIR`, `task rm`, start, clean up. | PR 57 and 62 | Copy by hand | A `just review-pr <n>` recipe that does exactly that |
| G6 | **TLS interception breaks Rust crates with bundled roots.** Norton's SSL scanning re-signs HTTPS; the OS store trusts it, `fastembed`'s downloader does not. | cosine probe | Fetch model files with `curl`, load locally | `sbxm doctor` could probe one HTTPS host and name the issuer when it isn't a public CA |
| G7 | **No place for downloaded assets.** `<config dir>` holds `config.toml` and profiles only. | cosine design (decision 166) | `model_dir` option, files fetched by hand | `sbxm models fetch` (a follow-up of #63) and a `models/` folder in the config dir |
| G8 | **Sandbox disk lives on C:, `doctor` doesn't check it.** A review sandbox takes about 7 GB there while it exists; the machine ran out twice. | PR 57 runs | Freed space by hand | `sbxm doctor` reads the drive of Docker's sandbox state and warns under about 10 GB |
| G9 | **Integration tests that need the host or the network can't run in a task sandbox** (a real model, real `sbx`). They are `#[ignore]`d and nobody runs them in a loop. | cosine design | Ignored tests with an env var, run by hand | `just real-test` already exists for `sbx`; add `just model-test` and list both in the PR template |
| G10 | **Unix-only code can't be tested from the Windows host.** The process-group kill and the symlink tests ran only through a WSL clone. | PR 57 M-2 | `git fetch` into a WSL clone, `cargo test` there | A `just test-linux` recipe doing that fetch-and-test |

## Friction in the working environment (not sbxm's code)

- `git` on Windows intermittently answers `Permission denied` on `.git/objects` (an indexer or antivirus holding a
  file); `git add` can stop halfway and a following `git commit` then commits a partial index. A wrapper that
  retries and refuses to commit when the add failed would remove a real way to lose work.
- The bash hook that blocks `\\` stops any command containing a Rust line continuation in a string. Edits with
  such strings go through the Write tool or a script file.
- Long test runs hit the 10 minute tool timeout and move to the background; a recipe that always runs them in the
  background and reports once would be calmer.

## Where a script could define the process

1. **Start a milestone slice from an issue:** file the issue (from a template with the acceptance-criteria
   checklist), `task start`, `task review`, `task finish`, `task rm`. Everything but the issue text is a fixed
   sequence: a `just slice <issue>` recipe.
2. **Review a PR:** G5's `just review-pr <n>`.
3. **Spikes:** a throwaway project plus an extra egress host plus a cleanup that proves the real secrets are
   untouched (S9 is the first such spike; its notes are in `sdlc/spikes/S9.md` once merged).
4. **After a review:** copy `review.md` into `sdlc/reviews/` with prefixed ids, file the should-fix findings,
   write the resolution line. Done by hand three times so far (PR 57 rounds 1-3, PR 62); `task file-findings`
   covers only the filing.
