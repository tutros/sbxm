# Milestone 4: `sbxm task` sandboxes on Docker Cloud Sandboxes

Planned 2026-10-23 with the user (issue #310, decisions 252-261, one answer each), from spike S14
(`sdlc/spikes/S14.md`, decision 248) and spike S17 (`sdlc/spikes/S17.md`, decision 255). The plan-level choices
were confirmed on 2026-10-24 (decisions 262-265).

## Goal

A repo can have its `sbxm task` sandboxes, workers and reviewers, run on Docker Cloud Sandboxes (`sbx --cloud`)
instead of the local machine [252]. A repo opts in with `[sandbox] location = "cloud"` in `sbxm-task.toml`, and
`--cloud` or `--local` overrides that for one command; a task remembers where it runs [253]. A cloud sandbox gets
the profile's two kits at create [254], a strict profile has its build hosts removed before the agent starts [255],
provider logins stay global while every other secret is scoped to its sandbox [256], and the pipeline keeps its
phases, with bundles in and out by `sbx --cloud cp` [257]. Every cloud sandbox has a TTL and a recorded lifetime
[259], [263], [265]. When the cloud can't be used, sbxm refuses before any write and never falls back to local [261].
Repos
without the key work exactly as today.

Deferred, each with its trigger where the answers give one:
- a cloud mode for `sbxm run`, `new` and `open` [252]: later, no trigger named;
- cloud templates [254]: when cloud setup takes over about 2 minutes a task;
- parallel orchestration (`--parallel`) [258]: only if the after-merge experiment with 2, then 3, background
  `task run`s justifies it;
- a daily budget [259]: a surprising bill.

## Plan-level choices (confirmed 2026-10-24, decisions 262-265)

All of these are answered: the user confirmed each on 2026-10-24 (decisions 262-265), and P18 and P19 were dropped.

- **P1. The `location` key's values and default.** [253] names `[sandbox] location = "cloud"`. Options: (a)
  `"local"` or `"cloud"`, default `"local"`, any other value an error naming the file (like every unknown key in
  `sbxm-task.toml`); (b) `"cloud"` only, with absence meaning local. **Recommend (a):** a repo can then write its
  choice either way, and the error matches the file's other keys. Needed by slice 1. **Confirmed:** (a) [262].
- **P2. Fixtures for the cloud parsers.** [260] says the parsers are tested on S14's captured output, but the S14
  write-up holds prose, not raw output, and `src/backend/fixtures/` has none from the cloud. Options: (a) one short
  capture run by hand before slice 1 (a default-shape `shell` sandbox, `--ttl 1h`, cents), saving the cloud's `ls
  --json`, `policy ls --json`, `secret ls --json` output, the not-logged-in message and the `rm` "temporarily
  unavailable" message, and trying the points S14 left open: whether `exec`, `cp` and `rm` take the name or only the
  `sbx_...` id, whether `exec -w` and `--skills` are accepted with `--cloud`, what `--ttl` accepts beyond `1h`,
  whether `--cpus`/`-m` pick the shape, and whether `rm` also removes the sandbox-scoped secrets as it does locally
  (S1); (b) write fixtures from S14's prose and check them only at slice 9's real test. **Recommend (a):** the
  parsers and command lines would otherwise be built on guesses, and a wrong guess shows up only at the end.
  **Confirmed:** (a) [262]. **Done 2026-10-24:** `sdlc/spikes/m4-p2-capture.md`, fixtures `src/backend/fixtures/sbx-cloud-*`;
  `rm` does not remove a sandbox's scoped secrets in the cloud, and `--skills` is refused with `--cloud`.
- **P3. The cloud backend: a second backend or a mode.** Options: (a) `SbxBackend` gains a location, and a task
  gets the local or the cloud instance, every call prefixed with `--cloud` in the cloud one; (b) a new
  `SbxCloudBackend` implementing `SandboxBackend`; (c) a location parameter on every trait method. In each case the
  trait gains copy in and out (`sbx --cloud cp`) and rule removal (`policy rm`/`policy deny`), and `CreateSpec`
  gains a TTL and an optional workspace (a cloud create has none, S14 Q1). **Recommend (a):** the command lines
  differ mostly by one flag, and a task already passes one `&dyn SandboxBackend` everywhere, so callers don't change.
  **Confirmed:** (a) [262].
- **P4. Where a cloud task's clone lives.** Options: (a) `$HOME/work/<task id>`, the path read once with `exec
  printenv HOME` and recorded in `task.json`; (b) `/home/agent/work/<task id>`, written into the code (S14 Q5 saw
  `/home/agent` as the agent's home; that it is always so is **inferred**); (c) the repo's own name under `$HOME`.
  **Recommend (a):** nothing assumes the home folder, and the recorded path is what `task gates` and `resume` use.
  **Confirmed:** (a) [262].
- **P5. What "the phase's time limit" is, and when a cloud worker sandbox goes.** [259] sets `--ttl` to a phase's
  time limit plus 30 minutes. Locally the worker sandbox lives from `task start` through every fix round to `task
  rm`, across several commands. Options: (a) TTL per phase: the worker phase is `[worker] time_limit` plus every
  sandbox gate's `[gates] timeout` plus 30 minutes, a review round `[reviewer] time_limit` plus 30 minutes, a fix
  round like the worker phase, `task gates` the gate timeouts plus 30 minutes; each phase that ends with its commits
  in `repo.git` removes its cloud sandbox, and the next phase that needs a worker sandbox creates one from the
  recorded profile and a bundle of the task's branch, as `task resume` rebuilds a gone one today; (b) one TTL for
  the whole task (the worker's limit plus `fix_rounds` times the reviewer's and worker's limits, plus 30 minutes),
  the sandbox kept until `task rm` as locally; (c) like (a) but the role's time limit only, gates not counted.
  **Recommend (a):** it follows [259]'s wording, a cloud sandbox isn't paid for while the task waits for the user,
  and the commits are already safe in `repo.git`. Needed by slice 3.
  (d), the user's own option: the cloud worker sandbox lives through the task's automatic run (worker, sandbox gates,
  review rounds, fix rounds) and is removed when the task stops (ready, or stopped for any reason). `task resume`
  rebuilds it from the recorded profile and a bundle of the task's branch when needed, as it rebuilds a gone worker
  sandbox today [118]. Its TTL: the worker's time limit + every sandbox gate's timeout + `fix_rounds` x (the
  reviewer's time limit + the worker's time limit + every sandbox gate's timeout) + 30 minutes. Reviewer sandboxes
  stay one per round, each with a TTL of the reviewer's time limit + 30 minutes. Why: fix rounds keep a warm build (a
  cold build is 233 s on the large shape, S14 Q5), and nothing is billed while a task waits for the user (up to
  2h25m from open to merge in the loop trial). **Confirmed:** (d) [263]; (a)-(c) were the options considered.
- **P6. The removal retry.** [259] says up to 3 retries. Options for the wait: (a) 5 s, then 10 s, then 20 s; (b) a
  fixed 10 s. Only an error that names a retry ("temporarily unavailable", S14 Q5) is retried; any other error is
  reported at once. **Recommend (a),** through an injected sleep as Jev's retries do [241]. The cause list is P2's
  captured message. **Confirmed:** (a) [262].
- **P7. How `task.json` records location and lifetimes.** Options: (a) a task-level `location` (absent means
  `local`) and an append-only `sandboxes` list, one entry per created sandbox: `name`, `role` (`worker`,
  `reviewer`, `fix`), `location`, `created_at`, `ttl_s`, `removed_at` (null while it exists or when removal
  failed); (b) the same fields on each `Agent`, which a new review round or a rebuilt worker overwrites. **Recommend
  (a):** a retro needs every sandbox's hours, and (b) loses all but the last of each role. How the list outlives
  `task rm` is P18. **Confirmed:** (a) [265].
- **P8. A profile's other secrets: where the value comes from.** [256] scopes every secret a profile names, other
  than the provider login, to the sandbox. S14 Q2 found a cloud custom secret takes a literal value only (no
  `--command`), and `sbx` can't read a local value back (S4). Options: (a) in this milestone, refuse a cloud task
  whose profile names a secret other than a provider OAuth login, naming the secret and `--local`, until a value
  source is decided (none of this repo's profiles does: `sbxm-dev` names only `anthropic`); (b) run a host helper
  and pass its output as the literal value, which puts the value on `sbx`'s command line on the host; (c) the user
  stores each one in the cloud by hand before every task. Also: (d) a profile naming the *other* provider's login
  (`anthropic` for a `codex` worker) is checked at global scope like the harness's own. **Recommend (a) and (d):**
  (b) is a new trust question for the user, (c) can't work for a sandbox name made at start. The scoping code is
  still built and tested with a fake value (slice 4), so lifting (a) later is a small change. **Confirmed:** (a) and (d)
  [262].
- **P9. The runtime list a strict cloud check accepts.** [255] keeps only the runtime list. Options: (a) the
  harness's own provider hosts, a fixed list per harness in code ([221]'s six OpenAI hosts for `codex`; for
  `claude`, `api.anthropic.com`, `claude.ai` and `console.anthropic.com`, the Anthropic hosts in S17's built-in
  list, **inferred** to be enough), plus `[template].allow`; (b) `[template].allow` alone, the user listing the
  provider hosts there. Removal by (i) `policy deny` per host, as [221] does at create locally, or (ii) `policy rm`.
  **Recommend (a) with (i):** the check stays the shape of [232], and a deny rule wins over any allow rule added
  later, which an `rm` doesn't promise. Slice 9's real test proves the agent still answers.
  [255] names the worker; the same removal before a strict reviewer or fix-round sandbox's agent starts is part of
  this choice. **Recommend yes,** since [200](b) holds for every sandbox of a strict profile. **Confirmed:** (a) with
  (i), for strict worker, reviewer
  and fix-round sandboxes alike, each sandbox's denied hosts written to the task's `run.log` [264].
- **P10. A non-strict `[template]` profile in the cloud.** There are no cloud templates [254]. Options: (a) build
  from the kits as for any profile, with one line saying cloud templates aren't used; (b) silently. **Recommend
  (a),** in the spirit of [200]'s one warning for a non-strict fallback. **Confirmed:** (a) [262].
- **P11. The free-disk check for a cloud task.** [194] refuses every sandbox-creating command under
  `min_free_disk` on the drive holding `sbx`'s local sandbox state, which a cloud sandbox doesn't use. Options: (a)
  skip the check for a cloud create; a cloud task still writes `repo.git` and host-gate checkouts under the base
  dir; (b) keep it. **Recommend (a),** with `--ignore-disk` still accepted. This changes [194]'s scope, so it is a
  new decision if confirmed. **Confirmed:** (a) [262], narrowing [194]'s scope for a cloud create.
- **P12. A minimum `sbx` version for the cloud.** Options: (a) 0.46.0, the version S14 and S17 ran, a constant like
  [213]'s, checked only for a cloud command; (b) none. **Recommend (a).** **Confirmed:** (a) [262].
- **P13. How `doctor` finds a cloud repo and matches `/tmp`.** [257] has `doctor` warn when a cloud repo's gates name
  `/tmp`. `doctor` doesn't read `sbxm-task.toml` today. Options for which repo: (a) the current directory's repo, when
  it has an `sbxm-task.toml` with `location = "cloud"`; (b) every repo a task record names. For the match: (i) a gate
  command containing `/tmp/` or ending in `/tmp` as a word; (ii) any `/tmp` substring. **Recommend (a) with (i):**
  `doctor` stays a check of where the user is, and (i) doesn't warn about `/tmpfoo`. **Confirmed:** (a) with (i) [262].
- **P14. Which commands take `--cloud`/`--local`.** Options: (a) only the commands that start a task, `task start`,
  `task run` and `task review --pr N`; every other command, `task review --issue/--spec` included, uses the
  location recorded in `task.json` and has no such flag; (b) every `task` command takes them, and one that
  contradicts the recorded location is refused. **Recommend (a):** [253] makes the record the truth once a task
  exists, and a flag that can only be refused adds nothing. **Confirmed:** (a) [262].
- **P15. `--workers N` in the cloud.** `task start --workers N` already runs workers in parallel today. Options: (a)
  unchanged in the cloud; (b) refused above 1 in the cloud until [258]'s experiment. **Recommend (a):** [258] rules
  out new parallel code, not code that exists, and the experiment can use either path. **Confirmed:** (a) [262].
- **P16. This repo's gate path.** [257] changes this repo's `/tmp/target` to `$HOME/target`, but no `sbxm-task.toml`
  is tracked in this repo today (the gates live in the user's copy). Options: (a) the docs slice adds the file to
  the repo with the cloud-safe path; (b) the user edits their own copy and the docs show the line. **Recommend (b):**
  adding the file changes what every checkout of the repo runs, which is the user's call. **Confirmed:** (b) [262].

- **P17. What `task status`, `status` and `doctor` do when the cloud can't be read.** [259] has `status` and
  `doctor` list the running cloud sandboxes; it doesn't say what an unreadable cloud (not logged in, `sbx --cloud
  ls` failing) does to them. Today `task status` and `status` never call `sbx` (`src/commands/task_status.rs`,
  `src/commands/status.rs`), and `doctor` records an unreadable local `sbx` as a failed check
  (`src/commands/doctor.rs`). Options: (a) `task status` keeps reading only `task.json` and shows the recorded
  location, with no cloud call; `status` prints one line saying the cloud couldn't be read in place of its cloud
  rows and exits 0; `doctor` shows a warning, not a failed check; (b) as (a), but `doctor` fails the check, as it
  does for the local `sbx`; (c) as (b), and `status` exits non-zero too; (d) as (a), but `doctor` fails the check
  only when the current repo is a cloud repo (P13 (a)), and warns otherwise. **Recommend (a):** `status --watch` is a
  view that shouldn't stop for a cloud outage, and a user who never uses the cloud (not logged in to it) must not
  see `doctor` fail. Needed by slice 8. **Confirmed:** (a) [262].
- **P18. How the sandbox lifetimes survive task removal.** P7 keeps the lifetimes only in `task.json` so a retro can
  total the sandbox hours [259], but `task rm` (and `task rm --pr`, and `--restart`'s removal) deletes the task folder
  with its `task.json` once the sandboxes are gone (`src/task/finish.rs`); only `task rm --merged` archives the
  records to `.sbxm/task-archive/<id>/` [195]. Options: (a) every removal of a task that has a cloud sandbox entry
  archives its text records first, reusing `.sbxm/task-archive/` and its collision rules from [195]; (b) the docs
  name an explicit export or retro step to run before `task rm`, and the hours of a task removed without it are lost;
  (c) each lifetime entry is also appended to a separate durable ledger under `.sbxm/`, a new file format. **Recommend
  (a):** the guarded archive exists already, and the normal cleanup then can't erase the promised measurement. This
  changes what `task rm` keeps, so it is a new decision if confirmed. Under (b), `task rm` doesn't refuse without
  the export; refusing until it is done would be another option for the user. Needed by slices 3 and 6.
  **Dropped** [265]: P7's lifetimes survive as every other retro number does today, in `task.json` and `run.log`,
  which `task rm --merged` archives [195]; a plain `task rm` loses them, as it already loses rounds and times, and
  `task rm`, `--restart` and the archive layout don't change. Why: merged tasks, almost all of them, are archived
  already; changing removal for a number nobody has needed yet goes against optimizing just in time (gap G62).
- **P19. The archive's identity for a reused task id (only under P18 (a)).** Task ids are reused: an issue or PR id
  comes from its number and a spec id from its canonical path (`src/task/record.rs`), and `--restart` removes and
  re-creates a task under the same id. [195]'s archive at `.sbxm/task-archive/<id>/` is never overwritten and takes
  a second archive only when every record is byte-identical (`src/task/merged.rs`, `same_records`), so under P18 (a)
  the second removal of a task with a reused id would refuse until the user moved the first archive aside. Options:
  (a) one immutable archive per task attempt, `.sbxm/task-archive/<id>/<attempt>/` (for example the time of the
  first entry of the task's `stages` list), with `--restart` archiving the old attempt before it removes it and,
  when the restart is restored, keeping that archive as the record of the attempt; (b) an append-only lifetime ledger instead, which is
  P18 (c); (c) one archive per id, the first attempt's kept and a later attempt's lifetimes lost, said in the docs.
  **Recommend (a):** every attempt's records survive and no earlier archive changes. It refines [195]'s path, so it
  is a new decision if confirmed, recorded before slice 3. Needed by slices 3 and 6. **Dropped** [265] with P18:
  there is no archive on `task rm`, so no archive per attempt; the reason is P18's.

## Structure (dependency order)

| Module | Responsibility |
|---|---|
| `src/task/config.rs` | `[sandbox] location` (P1); unknown values are errors. |
| `src/backend/` | cloud command lines (P3): `--cloud` prefix, `create` with `--ttl`/`--on-timeout delete` and no workspace, `cp` in and out, `policy deny` (P9 (i)), cloud `ls`, `policy ls` and `secret ls` parsers on fixtures (P2), the removal retry (P6); `FakeBackend` records the new calls. |
| `src/task/record.rs` | `location` and the sandbox lifetimes (P7); absent in old files means local and no entries. |
| `src/task/repo.rs` | the bundle of a task branch for a cloud clone (from `repo.git`, never from an agent's clone [159]); `fetch_bundle` reading from a host staging folder the copied-out file lands in. |
| `src/task/pipeline.rs` | one place that chooses the location (flag, then config, then record); cloud preflight [261]; `run_worker`, `bundle_and_fetch`, `collect`, `run_gates`, `open_review_workspace`, `run_review_agent` and `run_fix_round` through copy in/out instead of the mount; the worker and reviewer TTLs, the worker sandbox kept through the automatic run and removed when the task stops (P5 (d)); the strict removal and check (P9) before any agent starts, the denied hosts written to `run.log`; a `run.log` line at each cloud create and removal (P7). |
| `src/headless.rs` | the cloud's stream error after `timeout -v`'s lines is a timeout, never `ExecUnavailable` (S14 Q3); without those lines it stops the task as `sbx` not answering [261]. |
| `src/task/resume.rs`, `src/task/finish.rs`, `src/task/merged.rs` | the recorded location decides where a sandbox is rebuilt, found and removed. |
| `src/template.rs` | the per-harness runtime host list and the cloud rule check, beside `check_codex_rules` [232]. |
| `src/commands/status.rs`, `src/commands/doctor.rs` | running cloud sandboxes [259]; `doctor`'s `/tmp` warning (P13) and the cloud version (P12). |
| `tests/real_sbx_cloud.rs` (new) | the ignored real-cloud walk [260]. |

## Interfaces

### `sbxm-task.toml`

```toml
[sandbox]
profile = "sbxm-dev"
location = "cloud"   # [253]; "local" or absent: today's behavior (P1)

[gates]
# One list for both locations [257]: build under $HOME, never /tmp, in the cloud.
sandbox = [
  "cargo fmt --check",
  "CARGO_TARGET_DIR=$HOME/target cargo clippy --all-targets -- -D warnings",
  "CARGO_TARGET_DIR=$HOME/target cargo test",
]
```

### `task.json` additions (P7 (a))

```json
{
  "location": "cloud",
  "sandboxes": [
    { "name": "sbxm-task-<id>-claude", "role": "worker", "location": "cloud",
      "created_at": "2026-10-24T10:00:00Z", "ttl_s": 7200, "removed_at": "2026-10-24T10:41:12Z" }
  ]
}
```

`Agent.workspace` holds the in-sandbox clone path (P4) for a cloud task. A record without `location` is local.

The entries survive as every other record of the task does: `task rm --merged` archives `task.json` and `run.log`
[195], and a plain `task rm` removes them with the folder [265].

### `run.log` additions

- one line at each cloud create (the sandbox's name, role and TTL) and one at each removal [265];
- for a strict cloud sandbox, one line listing the hosts sbxm denied in it (P9) [264], the first thing to check when
  something fails unexpectedly in a strict cloud sandbox.

### Commands

| Command | Local (today) | Cloud |
|---|---|---|
| `task start`, `task run` | unchanged | location from `--cloud`/`--local`, else `[sandbox] location` (P14). Preflight [261], all before any write: `sbx` version (P12), cloud logged in, every provider OAuth login the task needs in the cloud (the harness's own and every provider service the profile names, P8 (d)) [256], no unsupported profile secret (P8), the profile's kits valid. Then `create --cloud --ttl --on-timeout delete --kit common --kit harness-<h>` with the worker TTL (P5 (d)), scoped secrets set before it [256], the strict removal and check (P9) before the agent, failing closed after create (below), bundle and prompt files in by `cp`, worker, sandbox gates, bundle out by `cp`, verify, fetch, host gates [257]; the worker sandbox is removed when the task stops (P5 (d)). |
| `task review --issue/--spec` | unchanged | the task's recorded location; a reviewer sandbox per round, each with the reviewer TTL (P5 (d)); a fix round in the task's worker sandbox, kept through the automatic run and rebuilt from a bundle of the branch if gone (P5 (d)). |
| `task review --pr N` | unchanged | location as for `start` (P14); the PR head goes in as a bundle from `repo.git`. |
| `task resume` | unchanged | in the cloud, always [261]: a gone sandbox (TTL, a failure) is rebuilt from the recorded profile and a bundle of the branch; preflight again first. |
| `task gates` | unchanged | in the recorded location; the worker sandbox rebuilt if gone, as `task resume` rebuilds it (P5 (d)). |
| `task finish` | unchanged | host-side only (`repo.git`); removes any cloud sandbox of the task still listed (one whose removal at the task's stop failed), with retries (P6), since under P5 (d) the worker sandbox goes when the task stops. |
| `task rm`, `task rm --merged` | unchanged | removes the task's cloud sandboxes (`sbx --cloud rm`, retried, P6) and its scoped secrets [256], then the folder; the lifetimes go with it as every other record does, and `task rm --merged` archives them first [195], [265]; a sandbox already gone counts as removed. |
| `task status`, `status` | unchanged | the task's location; `status` adds the running cloud sandboxes named `sbxm-` (`sbx --cloud ls`) [259]; what an unreadable cloud does is P17. |
| `doctor` | unchanged | also lists running cloud sandboxes, an unreadable cloud handled per P17; warns when the current repo is a cloud repo and its gates name `/tmp` (P13). |

### Preflight refusals [261]

Every refusal in this table is detectable before `create`: it comes before the first write, delete or backend call
that changes anything, and ends with "or run it here with `--local`".

| Cause | Message names |
|---|---|
| not logged in to the cloud | `sbx login` |
| a provider OAuth login the task needs missing in the cloud: the harness's own, or any provider service the profile names (P8 (d)) | `sbx secret set <provider> --oauth --cloud` |
| `sbx` older than the cloud minimum (P12) | the version found and the minimum |
| a profile secret with no cloud value source (P8 (a)) | the secret and the profile |

### After create: the strict-policy check fails closed [255]

The strict check can't be a preflight refusal: it reads the sandbox's own policy, which exists only after `create` and
the rule removals (P9). In order: scoped secrets set [256], `create`, one `policy deny` per host outside the runtime
list (P9 (a) with (i)), the denied hosts written to `run.log` [264], `policy ls`. If the list still allows a host
outside the runtime list, or sbxm can't read it, sbxm removes the sandbox (with P6's retries), then the task's scoped
secrets, and only then refuses, naming the host (or the unread list), as in [232]. This holds for strict worker,
reviewer and fix-round sandboxes alike [264]. No agent `exec` runs before the check passes. The task's record and folder
are left as a local strict `codex` start that [232] refuses leaves them today, and a removal that fails is reported as
P6 reports one.

A cloud failure mid-task (`sbx --cloud` not answering, an exec stream error without `timeout -v`'s lines) stops the
task the way `SbxUnavailable` and `ExecUnavailable` do today, and the message names `task resume`.

## Work order (vertical slices, each TDD, each ends with fmt, clippy, cargo test and a commit)

**Risky slices** (decision 162, reviewed in the background as each is committed):
- **1**: copying a file out of an agent-controlled sandbox onto the host is a trust boundary [159].
- **3**: cloud removal and its retries are destructive, and a TTL too short loses a worker's commits.
- **4**: scoped secrets are a security guard [256].
- **6**: `resume`, `finish` and `rm` find and remove cloud sandboxes (destructive).
- **7**: the strict removal and its check are the guard of [255].

The other slices are reviewed at the end of the milestone.

1. **Thinnest path: `task start --spec FILE --cloud`, non-strict, through `FakeBackend`.** The `location` key and the
   `--cloud`/`--local` flags (P1, P14); the backend's cloud command lines (P3) with `create` taking two kits and no
   workspace; the branch bundled from `repo.git`, copied in and cloned under the recorded path (P4); prompt files in
   and `result.md` and the bundle out by `cp` into a fresh host staging folder, each checked as one regular file
   under its cap before it is read; verify and fetch; sandbox gates by `exec` in the clone; `location` in
   `task.json`. Done when: a `tests/task_cloud.rs` case shows the backend log in order, `repo.git` holds the
   worker's commit, `task.json` says `cloud`, and a staged file that is a link or over its cap is refused.
2. **Cloud preflight and refusals** [261]: logged in, every provider OAuth login the task needs in the cloud (P8
   (d)), version (P12), the free-disk rule (P11), each refusal before any write, naming its fix and `--local`. Done
   when: one test per row of the preflight refusals table asserts the message and an empty backend log of changing
   calls and no task folder, plus the opposite-provider case: a `codex` task whose profile names `anthropic`, with
   `anthropic` missing in the cloud, is refused before any write, naming `sbx secret set anthropic --oauth --cloud`.
   The strict-policy refusal isn't one of them; it is slice 7's.
3. **Lifetimes**: `--ttl` and `--on-timeout delete` with the worker TTL and the reviewer TTL (P5 (d)), the removal
   retry with an injected sleep (P6), the `sandboxes` list in `task.json` and a `run.log` line at each cloud create
   and removal (P7, [265]); the worker sandbox kept through the automatic run, removed when the task stops (ready or
   stopped), and rebuilt from a bundle of the branch when a later command needs it. Done when: tests on the computed
   worker and reviewer TTLs, the worker sandbox removed when the task stops ready and when it stops on a failure, a
   removal that succeeds on the third retry, one that fails after the initial attempt plus three retries (4 calls in
   all) and is reported, the injected-sleep test observing 5, 10 and 20 seconds, in order, and a record whose entries
   add up to the sandbox hours.
4. **Scoped secrets** [256], P8: a non-provider secret refused (P8 (a)) or, with a fake value in the tests, set
   with `--sandbox <name>` before `create` and removed after the sandbox on every path, including a failed create.
   Done when: `tests/task_cloud.rs` cases on the `secret-set`/`secret-rm` log order.
5. **Reviews and fix rounds in the cloud**: `task review --issue/--spec` and `task run` (reviewer bundle in,
   `review.md` out by `cp`, the same staging checks as slice 1), fix rounds, `task review --pr N` with the PR head
   bundled from `repo.git`. Done when: a cloud `task run --spec` through `FakeBackend` reaches `ready` with the
   expected log.
6. **`resume`, `gates`, `finish`, `rm` by the recorded location**, and mid-task failures: a gone sandbox rebuilt in the
   cloud by `resume`; `rm` and `rm --merged` removing cloud sandboxes and scoped secrets; the stream error after
   `timeout -v`'s lines classified as a timeout (S14 Q3's captured text), and without them as `ExecUnavailable`;
   the lifetime entries archived by `task rm --merged` with `task.json` and `run.log` and removed with the folder by
   a plain `task rm`, with `task rm`, `--restart` and the archive layout unchanged [265]. Done when: tests per
   command, including a record that says `cloud` while the config says `local`.
7. **Strict mode in the cloud** [255], P9, P10: for a strict worker, reviewer or fix-round sandbox, create with the
   build allow list, then one `policy deny` per host outside the runtime list (the harness's fixed provider hosts
   plus `[template].allow`), the denied hosts written to `run.log` [264], then `policy ls` parsed and checked as in
   [232]: any other allowed host, or a
   list sbxm can't read, removes the sandbox, then the scoped secrets, then refuses, naming the host ("After create:
   the strict-policy check fails closed" above). Done when: tests on the parsed S17-shape list, a kept host, a host
   left over, and an unreadable list, the denied hosts in `run.log`, and a reviewer sandbox checked like the
   worker's; for the last two the backend log is, in order, `secret-set`, `create`, the
   removals, `policy ls`, the sandbox `rm`, `secret-rm`, then the refusal; no agent `exec` happens before the check
   passes.
8. **`status` and `doctor`**: running cloud sandboxes [259], `doctor`'s `/tmp` warning (P13) and the cloud version
   (P12), and the unreadable cloud per P17's answer. Done when: `tests/doctor.rs` and status golden cases, including
   the cloud not answering, assert P17's chosen output and exit status (or doctor check result) for `task status`,
   `status` and `doctor`, each failing before its implementation.
9. **Real-cloud test** [260]: an ignored `tests/real_sbx_cloud.rs`, gated on its own environment variable (for
   example `SBXM_REAL_CLOUD=1`) besides `SBXM_REAL_BASE_DIR`, on the small shape: one tiny spec task through
   `task run`; egress accepting all three refusal shapes (DNS failure, connection refused, proxy 403, S14 Q1); a
   fake scoped secret's value found in no file and no env var; the TTL in the create; the bundle in and out; a
   strict sandbox's list after the removal; removal confirmed by `sbx --cloud ls` and `policy ls`. Done when: it
   passes by hand once, recorded with time and the bill's change.
10. **Docs**: README (the key, the flags, the refusals, the cost note), `docs/workflow.md` (build under `$HOME` in
    the cloud [257], the TTL [259], [263], the parallel experiment [258], and a troubleshooting line: when something
    fails unexpectedly in a strict cloud sandbox, check first whether a host listed as denied in the task's `run.log`
    is actually needed [264]), and the `$HOME/target` line for the user's own `sbxm-task.toml` (P16 (b)). Done when:
    README and `docs/workflow.md` describe every row of the command table above.
11. **End-to-end check** (below). Done when: every step passes and the record is in `sdlc/reviews/`.

## End-to-end checklist (real `sbx --cloud`, small shape)

1. The user reads the bill before starting.
2. `sbxm doctor`: the cloud version passes, no running cloud sandboxes, the `/tmp` warning shown for a repo whose
   gates still say `/tmp/target` and gone after the change to `$HOME/target`.
3. Logged out (or with the provider secret removed): `task start --cloud` refuses before any write, naming the fix
   and `--local`.
4. `sbxm task run --spec <tiny spec> --cloud` on a scratch repo: `ready`; `task.json` records `cloud` and one entry
   per sandbox with its TTL, and `run.log` a line at each create and removal; `sbx --cloud ls` shows the sandbox
   while it runs with the TTL set, and none of the task once it is `ready`.
5. The same with a strict profile: `sbx --cloud policy ls` on the worker shows only the runtime list before the
   worker starts, `run.log` lists the denied hosts, and a build host refuses in one of the three shapes.
6. Kill `task run` during the worker, then `task resume`: the task continues in the cloud.
7. `task rm`: `sbx --cloud ls` has no sandbox of the task, `policy ls` has no rule scoped to it, no scoped secret is
   left, and the global policy is as it was.
8. `sbxm status` during step 4 lists the running cloud sandbox; afterwards, none.
9. `tests/real_sbx_cloud.rs` passes.
10. The user reads the bill after; the change is recorded with the total sandbox hours, read from the task's
    `sandboxes` entries before step 7's `task rm`, which removes them with the folder [265].

## Out of scope

`sbxm run`, `new` and `open` in the cloud [252]; cloud templates [254]; parallel orchestration and `--parallel`
[258]; a daily budget [259]; a value source for non-provider secrets beyond P8 (a); cloud credentials in GitHub CI
[260]; any quiet fallback to local [261].

## Risks

1. **Three shapes of egress refusal** (S14 Q1: DNS failure, connection refused, a proxy 403), none of them
   explained. A test or a gate that expects 403 misreads the cloud. Mitigation: the real test accepts all three
   [260]; nothing in sbxm's own logic depends on the shape.
2. **The timeout's stream error** (S14 Q3): a timed-out command with an orphaned child ends in exit 1 and
   `RunExecSession: stream error`, not 137. Read as `sbx` unavailable, it would stop a task that only timed out.
   Mitigation: slice 6 classifies by `timeout -v`'s lines first, on S14's captured text.
3. **Removal can fail once** (S14 Q5: "temporarily unavailable"). Mitigation: up to 3 retries (P6) [259], the TTL as
   the backstop, `status`/`doctor` listing what is left.
4. **`/tmp` is small** (S14 Q5: a tmpfs of half the memory, filled by a cargo target folder). Mitigation: the clone
   under `$HOME` (P4), one gate list building under `$HOME`, `doctor`'s warning [257].
5. **Global provider secrets reach every cloud sandbox** (S14 Q7). A `claude` sandbox also gets the OpenAI login.
   Mitigation: the value never enters the sandbox (S14 Q2, Q7), egress allows the harness's hosts only in strict
   mode [255]; in a non-strict sandbox the agent's built-in list can include other providers' hosts (S17), which the
   end-to-end check records.
6. **The agent's built-in host lists** are wide even for `shell` (S17: Anthropic, OpenAI, GitHub, Google, Mistral,
   Nebius, OpenRouter, xAI). Strict mode must remove all of them except the runtime list. Mitigation: the check reads
   the whole list after removal and fails closed [232], [255]; that every built-in host can be removed is shown for
   one host only (S17, **inferred** for the rest).
7. **A forgotten sandbox costs money.** Mitigation: every sandbox has a TTL with `--on-timeout delete` [259], the worker
   sandbox removed when the task stops (P5 (d)) [263], `status` and `doctor` list running cloud sandboxes, and the user
   reads the bill.
8. **A TTL too short loses uncollected work.** A sandbox deleted by its TTL before the bundle comes out takes the
   worker's commits with it. Mitigation: the worker TTL covers the worker, every sandbox gate and every fix round's
   reviewer, worker and gates, plus 30 minutes (P5 (d)) [263]; `resume` rebuilds from what `repo.git` holds and says so.
9. **Copying out of an agent-controlled sandbox** (`sbx --cloud cp`): what `cp` does with a link or a folder at the
   source path is unknown. Mitigation: a fresh host staging folder per copy and the same one-regular-file and cap
   checks as `read_agent_file` before anything reads it (slice 1).
10. **Unverified `sbx` details** (name or id for `exec`/`cp`/`rm`, `-w`, `--skills`, `--ttl` units, shape flags).
    Mitigation: P2's capture run before slice 1.
11. **A denied host that is needed** [264]: sbxm removes hosts from a strict cloud sandbox, so a host the agent or a
    gate needs shows up as an unexpected agent or gate failure. Mitigation: check the denied hosts in `run.log`
    first; the docs' troubleshooting line says so (slice 10).
