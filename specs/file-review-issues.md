# Task spec: `scripts/file-review-issues.ps1` (host-side filing of review findings)

Written 2026-09-30 for a fresh session to build. Follows `sdlc-implementation` rule 5 (a written spec for work the
user doesn't check step by step). Decision 134 records why this exists and where it lives.

## Start here

1. Read, in order: this file, `decisions.md` 79, 86–88, 134, `.claude/skills/sdlc-code-review/SKILL.md` section 8
   (the issue template and the "Review file format" this script parses), `.claude/skills/sdlc-implementation/SKILL.md`,
   and `AGENTS.md`.
2. Branch from `m2a-implementation` (it has the newest skills and decisions): `git switch -c feat/file-review-issues`.
   Never push to `main` (decision 86). Do not touch `scripts/issue-workers.ps1` or `.psm1` (decision 88: frozen).
3. Work the tasks below in order, test first, one commit per green step. The Pester suite is `just script-test`.
4. The real sample the parser must handle is `scripts/tests/fixtures/review-m2a-codex.md` (Codex's actual review of
   milestone 2a). Do not edit that fixture; `reviews/2026-09-30-milestone-2a.md` is the same text, the file the
   script will later write issue numbers back into.

## Why

Reviews run in sandboxes with no GitHub access (decision 79 and the skill's access check), so their findings stay in
`reviews/<date>-<scope>.md` marked `Issues: pending`. Filing them by hand is slow and inconsistent: the template has
ten parts, dependencies must be linked both ways, and the repo is public so content must be scrubbed. This script
turns a review file into issues, deterministically and repeatably, from the host where `gh` works.

## Environment facts (verified 2026-09-30 unless marked)

- PowerShell 7.6.6; Pester 5.5.0 installed (plus 5.1.1 and 3.4.0: import with `-MinimumVersion 5`); `just` and
  `gh` 2.83.2 on PATH. The host is logged in to `gh` as `tutros`; issue #32 was filed that way. Inside a sandbox `gh`
  has no login and `origin` may be a local path: that is exactly why the script exists.
- Repo: `tutros/sbxm`, public. Labels that exist: `must-fix`, `should-fix`, `question` (plus GitHub's defaults). The
  script must not create labels. Issues #1 to #32 exist; do not assume the next number, read it from `gh`.
- `just script-test` runs `Invoke-Pester scripts/tests -Output Minimal -CI`. Existing tests mock native commands by
  defining functions in the module scope: read `scripts/tests/PrReview.Tests.ps1` and the `Invoke-Native` helper in
  `scripts/issue-workers.psm1` for the project's pattern, and reuse it.
- The Bash tool turns every double backslash into one and a hook blocks them. Write files with the Write and Edit
  tools, pass bodies to `gh` with `--body-file` (temp files, deleted afterwards), and use the PowerShell tool for
  anything with Windows paths.
- Files in this repo are CRLF on disk and UTF-8. Don't rewrite them through a script that reads with the default
  Windows code page; use Edit/Write.
- Run one heavy command at a time. Overlapping `cargo` runs corrupted `target/` once (only relevant if you run
  `just check`; the script work itself needs no Rust).
- The worker flow's `review.md` (first line `Must-fix findings: N`, written by `issue-workers.ps1`) is a different,
  PR-scoped format. Leave it alone. This script handles the milestone and slice review files in `reviews/`. Unifying
  the two is M2b's job.
- Unverified: whether `gh issue list --search` finds text inside an HTML comment. Do not rely on it; list issues as
  JSON and scan the bodies (see Idempotency).

## Interface

```
./scripts/file-review-issues.ps1 -Review reviews/2026-09-30-milestone-2a.md [-Repo tutros/sbxm] [-Create]
    [-StandardCriteria] [-KeepPaths] [-Only M2A-1,M2A-2]
```

- Default is a **dry run**: parse, check access, print every issue it would create (title, label, body) and what it
  would write back. Nothing is created or edited. `-Create` files the issues. The repo is public, so publishing must
  be explicit.
- `-Repo` defaults to the name `gh repo view --json nameWithOwner` returns for the current checkout.
- `-Only` files only the listed finding ids.
- `-StandardCriteria` adds the standard acceptance criteria to findings that have none (see Parsing).
- `-KeepPaths` turns off the personal-path replacement (see Scrubbing).
- Exit codes: 0 all done; 1 nothing filed (parse error, access failure, refused content); 2 some filed, some not.

## The review file format

The canonical format is defined in the skill (section 8, "Review file format"). The real Codex output differs from
it in small ways, and the parser must accept both. Canonical:

```
Scope: `<base sha>..<head sha>` ...          first paragraph line naming the reviewed commits
Issues: pending (...)                         or: Issues: M2A-1 #33, M2A-2 #34

## Must fix                                   also "## Should fix", "## Question" or "## Questions"
### M2A-1 - <title>                           ids are any non-space token; "-", an em dash or ":" separate id and title
**Where:** `src/git.rs:50-72`, `src/run/diff.rs:23-34`
**What happens:** ...
**Why it matters:** ...
**Fix:** ...                                  accepted aliases: **Smallest fix:**; for questions **Recommendation:**
**Depends on:** M2A-2, #12                    optional; "none known" means none
**Related:** ...                              optional
**Acceptance criteria:**                      optional in Codex's output, required by the template
- [ ] ...
```

Rules:

- **Label** comes from the `##` section the finding sits under (`Must fix` is `must-fix`, `Should fix` is
  `should-fix`, `Question`/`Questions` is `question`). Case and trailing colons are ignored.
- Text under `##` headings that hold no `###` findings (Summary, Verification and checklist notes, a `Nit:` bullet)
  is not filed. Say so in the dry-run output ("not filed: 2 sections, 1 nit").
- **Title** of the issue is `<id>: <title>`.
- **Field names are matched case-insensitively** and may be followed by text on the same line or by a paragraph or
  bullets below. A finding is **invalid** (reported with its id and what is missing, never silently dropped) when it
  lacks `Where`, `What happens` or `Why it matters`, or, for must-fix and should-fix, `Fix`. A question needs
  `Where`, `What happens`, `Why it matters` and a `Recommendation` or `Fix`.
- **Missing acceptance criteria.** The template requires them and Codex's review has none. Default: the finding is
  invalid with the message `M2A-1 has no acceptance criteria; add them to the review file, or rerun with
  -StandardCriteria to file it with only the standard ones`. With `-StandardCriteria` the body gets the standard
  checklist from the skill (test fails before and passes after, fmt/clippy/test pass, docs updated or "no
  user-visible change") and a first line `- [ ] <the specific check is missing from the review: add one before
  working this issue>`. Questions get no criteria.
- **Where** tokens that look like `path:line` or `path:a-b` (inside backticks or not) become permalinks
  `https://github.com/<repo>/blob/<head sha>/<path>#L<a>-L<b>`, using the head sha of the `Scope:` line (the sha after
  `..`; a 7-or-more-hex-character string is accepted and resolved with `git rev-parse` when short). If no sha is
  found, leave the text as is and print a warning. Paths with no line keep the path and link to the file.
- **Depends on / Related** hold finding ids (`M2A-2`) or existing issue numbers (`#12`). Ids are rewritten to the
  created issue's `#n` after creation (see Creating).

## The issue body

Exactly the skill's template, in this order: **Where**, **What happens**, **Why it matters**, **Fix** (a question
has **Options and recommendation** instead), **Depends on**, **Related**, **Acceptance criteria**, **Review**
(`reviews/<file name>`, finding id, and the reviewed commit range). A hidden marker
`<!-- review-finding: <review file name>#<id> -->` is the last line. Missing Depends on becomes `none known`.
Missing Related is omitted.

## Idempotency

Before creating anything, list existing issues once: `gh issue list --repo <repo> --state all --limit 1000 --json
number,title,body` and scan the bodies for each finding's marker. A finding whose marker exists is **skipped** and its
number is reused for links and the write-back. Re-running after a failure or after hand edits never duplicates. If
the list has exactly the limit's worth of issues, stop with an error (the scan could be incomplete).

## Creating (with `-Create`)

1. Access checks first, as skill section 8 says: `git remote get-url origin` names a GitHub repo (or `-Repo` is
   given), `gh repo view` answers, `gh auth status` is logged in with `repo` scope, and each needed label exists.
   On failure print the skill's "pending" message and exit 1 without touching anything.
2. Validate and scrub **every** finding before filing any (all-or-nothing on content: one refused finding stops the
   run unless `-Only` excludes it).
3. File in dependency order (a finding after the ones it depends on). Bodies go through `--body-file` temp files.
4. After every issue exists, rewrite `Depends on` and `Related` ids to `#n` (an edit pass with `gh issue edit` where
   the number wasn't known at creation, for cycles or forward references). Write the link on **both** issues, as the
   skill requires: a `Depends on` on one implies a `Related` back-reference on the other.
5. Print a table: id, label, number, URL, `created` or `skipped (exists)`.

## Write-back

Replace the review file's `Issues:` line with `Issues: M2A-1 #33, M2A-2 #34, ...` (findings not filed keep
`pending`). Edit only that line; keep the file's line endings. A dry run prints the new line and changes nothing.

## Scrubbing (the repo is public)

- **Refuse** (finding is invalid, nothing is filed) any text matching a secret pattern: GitHub tokens
  (`ghp_`, `gho_`, `ghs_`, `github_pat_`), `sk-` followed by 20 or more characters, AWS `AKIA` keys, `Bearer ` followed
  by a long token, private-key headers, and `password =`/`token =` assignments with a value. Print the finding id
  and the line number, never the value.
- **Replace** personal paths by default: `C:\Users\<name>\...` and `/home/<name>/...` become `~\...` / `~/...`;
  warn for each. `-KeepPaths` turns this off. Project paths such as `E:\sbxm-projects\...` are left alone.
- **Warn** on e-mail addresses (don't change them).

## Tasks, methods and acceptance criteria

Each task is one or a few red-to-green commits. Every criterion needs a Pester test named in the commit.

| # | Task | Acceptance criteria |
|---|---|---|
| T1 | Parser: review text to findings | The Codex fixture yields exactly 4 findings: M2A-1 and M2A-2 `must-fix`, M2A-3 `should-fix`, M2A-Q1 `question`, each with id, title, label and the fields present in the file. The Summary, Verification and Nit text is reported as not filed. A canonical-format sample and the Codex variant give the same structure. Missing required fields name the finding and the field. Headings with an em dash, a hyphen and a colon all split id from title. |
| T2 | Where links and body rendering | `src/git.rs:50-72` becomes the permalink at the head sha from the Scope line; a short sha is resolved; no sha gives a warning and plain text. The body has the template parts in order, `none known` for a missing Depends on, and the marker as the last line. A question body uses "Options and recommendation". Golden-file tests compare full bodies. |
| T3 | Acceptance criteria handling | No criteria: invalid with the exact message above. `-StandardCriteria`: the standard checklist plus the placeholder first line. A finding with its own criteria keeps them and gets the standard ones appended. Questions get none. |
| T4 | Scrubbing | Each secret pattern refuses the finding and names id and line, without echoing the value. `C:\Users\james\x` becomes `~\x` with a warning, `-KeepPaths` keeps it, `E:\sbxm-projects\x` is untouched. |
| T5 | Access checks and dry run | Each failure (no GitHub remote and no `-Repo`, `gh` not logged in, missing label) stops with a message naming the fix and exit 1, with no write call made (asserted on the mocks). The dry run prints every issue and the `Issues:` line and calls no `gh` write command. |
| T6 | Create, dependencies, idempotency | With mocked `gh`: issues are created in dependency order; `Depends on M2A-2` becomes `#<number>` in both bodies and the reverse `Related` is added; a second run creates nothing and reuses numbers; a run interrupted after 2 of 4 creations resumes with the other 2; a full issue list at the limit stops with an error; `-Only` files just those. |
| T7 | Write-back | The `Issues:` line becomes `Issues: M2A-1 #33, ...`; nothing else in the file changes (byte-compare the rest, CRLF kept); findings not filed stay `pending`; the dry run leaves the file untouched. |
| T8 | Docs and wiring | `AGENTS.md` code layout lists the script and module; `scripts/tests/` has the test files; `just script-test` passes from a clean checkout; the script's comment-based help matches the Interface section; `decisions.md` 134 is marked built. |
| T9 | Live check (needs the user) | See below. |

## T9: live check (Blocked until the user approves each step)

1. Dry run on `reviews/2026-09-30-milestone-2a.md` with the real `gh`: show the user the full output. No approval is
   needed for this step.
2. The user decides which findings to file (the review's own questions and must-fix items first need triage, and the
   file has no acceptance criteria: expect the `-StandardCriteria` path or edits to the review file by the user).
3. With the user's go-ahead: `-Create -Only <ids>` for one finding, check the issue in the browser or with
   `gh issue view`, run it again to confirm `skipped (exists)`, then file the rest.
4. Comment on decision 134 with the result. If the user doesn't approve step 3, record T9 as Blocked with the reason.

## Statuses

Every task ends as Done (with the test names and the commit), Partial (gap stated) or Blocked (reason stated). A
belief without a passing test is not Done.

## Budget and stop rules

- No `gh` write command (`issue create`, `issue edit`, `issue comment`) is run during development: the tests mock
  `gh`. The only real writes are T9 step 3, after approval.
- At most 3 red-to-green attempts per task. The same error twice after a fix: mark the task Blocked and stop.
- Stop and ask if: the skill's template or decision 79 would have to change beyond the "Review file format"
  section already written; the parser can't be made to accept the Codex fixture without a rule not written here; a
  task needs a new tool or module other than Pester and `gh`; or you would need to edit `issue-workers.*`.
- Never force-push, never push to `main`, never edit `.claude/settings*.json`.

## Side effects

Creates `scripts/file-review-issues.ps1`, `scripts/file-review-issues.psm1`, `scripts/tests/ReviewIssues*.Tests.ps1`
and more fixtures under `scripts/tests/fixtures/`. Edits `AGENTS.md` (code layout), `decisions.md` (134 status) and
the skill only if a rule here turns out wrong (say so in the commit). Temp files go in `$TestDrive` or the system
temp dir and are removed.

## Output contract

The work is commits on `feat/file-review-issues` and a final message with: each task's status and evidence, the
`just script-test` result, what the dry run on the Codex review printed, anything deferred, and the exact steps for
the user to run T9. Do not edit `decisions.md` beyond the status of 134; new decisions are proposed in the message.

## Follow-ups (not part of this task)

- The `sbxm-dev` profile (`profiles/sbxm-dev/profile.toml`) lists only the `anthropic` secret, so a Codex review
  sandbox created with it also needs `openai` stored and nothing says so. Add `openai` to its `secrets.services`, or
  give Codex reviewers their own profile (changes `issue-workers.ps1`'s default profile, so it waits for M2b or a
  user-approved bug-fix exception).
- M2b folds this script into an `sbxm` subcommand; keep the parser and renderer free of host-specific code so it moves
  over.
- Answering the milestone 2a review's findings (M2A-1 to M2A-Q1) is a separate piece of work, not this task.
