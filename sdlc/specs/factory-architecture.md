# Toward a software factory: planes, contracts and the foundation (planning draft, 2026-10-03)

Status: **proposals from an interview with the user, not decisions.** Nothing here is built. Related: `sdlc/jev.md`,
`sdlc/specs/jev-questions.md`, `sdlc/evals-workflow-notes.md` (gaps G1 to G26 are cited below as G-numbers).

## 1. The goal

The user's goal (2026-10-03): build as many tools as make sense to define a workflow for creating and maintaining
applications and systems, a **software factory**. sbxm is the first tool: launch sandboxes for an LLM and harness to work on
a problem, safely, so projects can be defined, problems solved and solutions validated. The user expects to work on
**several projects at once**, and to run in the cloud when the local machine has too few resources. All of this depends on
a stable base, so the base is the next thing to define.

## 2. The planes (proposed)

| Plane | Does | Today |
|---|---|---|
| **Execution** | Create sandboxes, run harnesses headless, enforce isolation (egress, secrets, timeouts), capture answers, diffs, transcripts and usage, tear everything down | sbxm: `new/open/...`, `run`, the sandbox half of `task` |
| **Workflow** | Move work through stages with gates and human checkpoints: issue to worker to gates to review to fix to PR | `sbxm task` (repo-centric, issue-centric) |
| **Decision** | Evaluate a response against criteria (evaluator) and choose a model for a task (router) | LLM judge, checks, cosine inside sbxm; Jev and routing not built |
| **Records** | The memory of the factory: run records, outcomes, price table, benchmark input, the learned routing table | plain files per run under `<base>/.sbxm/runs/`; no shared schema, no price table, no outcome store |
| **Later** | Planning and specs, release and deploy, operations | the `sdlc-*` skills are the manual version of planning |

Rule of thumb for what goes where: **whatever needs a sandbox stays in the execution plane** (the LLM judge, executable
checks); whatever only needs data and a model call can live outside it (a Jev evaluator, the router). A service that needed
sbxm to do its work would be circular.

## 3. Contracts between planes (draft shapes, to be settled)

Everything below is versioned (`schema: 1`) and carries the version of whatever produced it.

- **RunRecord** (execution to records): identity (project, task, pair), the sbxm build (version and commit), config hash,
  image or template version, harness and **exact model name**, task text and task features, timings, `Usage` with *every*
  figure the harness reports (fresh, cache-written, cache-read, output, reasoning) and the model, the diff, the answer, gate
  results, review findings count, and the accepted level (section 4 of `jev-questions.md`).
- **EvalRequest / EvalResult** (execution to decision and back): the task, one candidate (answer, diff, result text), the
  criteria (ids, kinds, weights, notes); back come per-criterion scores with confidence, the evaluator's name and model
  version, and its own usage. Never a raw upstream error body (spike S12: a 422 echoes the request).
- **RouteRequest / RouteResult**: the task text (and optional constraints: minimum acceptance, cost ceiling, deadline); back
  comes a ranked list of models with the expected acceptance rate, expected cost, the table version and the reasons.
- **Price table:** per model, dollars per million tokens for input, cached input and output, with an effective date; a
  run records the version it used.

The first transport is **a command with JSON on stdin and stdout** (zero operations, easy to test with fixtures), and the
same shapes can move to HTTP when a second consumer needs it. In sbxm that is one generic hook (for example
`[eval.external]`), not one config key per evaluator.

## 4. What the base needs

Each row: the need, the evidence from the work so far, where we stand.

| # | Need | Evidence | Today |
|---|---|---|---|
| F1 | **Reproducible environments.** Pinned toolchain and system packages baked in, and sandboxes that cannot change themselves (the user's strict mode). | A worker installed `pkg-config` itself, so its gates passed where a clean sandbox failed (G20). | Per-sandbox install steps; templates are spike S11, unfinished. |
| F2 | **Provenance.** Every result names the build, config, image and model versions that produced it. | A whole run was discarded because nobody could say which binary produced it (G21, G24). | Config hash only; `--version` prints `0.1.0` for every build. |
| F3 | **Resource accounting and placement.** Know what a sandbox costs (disk, CPU, memory) before starting it, refuse or queue when there is not enough, and choose local or cloud. | C: reached 0 GB three times; a task sandbox is up to 33 GB of nominal disk with no setting to move it (G18, G19). `sbx` has `--cloud`, `attach`, `move` between local and cloud, `ttl` and cloud-only volumes, so the cloud half exists in `sbx`. | `doctor` checks only the base dir. The backend is local only; no scheduler. |
| F4 | **A project model.** A registry of projects, each with its own policy, secrets and limits, and every identifier namespaced by project. | Tasks are defined per repo and ids are `issue-<n>` and `pr-<n>`, so separate `base_dir`s keep repos apart; the shared part is `sbx`'s one sandbox list, where a second sandbox with an existing name is refused (tested with a plain sandbox, G27). Deferred until a second repo is actually used. Tasks are found only under the config dir they were made with (G22). | One repo per working directory; sandbox names carry a project for `new` but not for `task`. |
| F5 | **Safe concurrency.** Locks and atomic updates for tasks, so two operators or sessions cannot corrupt a record. | Two runners on one task (G24); an open should-fix issue about concurrent gate commands overwriting one record (#58). | In-memory guard only. |
| F6 | **Resumable, recoverable workflows.** Every stage can be retried, resumed or taken over without starting again. | A fix round died because a sandbox was gone and nothing could resume (G17); a `ready` task cannot be re-gated or re-reviewed (G25); a finish cannot recover when the PR exists but its URL was not recorded (#60). | Restart from scratch is the only exit. |
| F7 | **Continuing work on an open PR.** | The independent review of PR 69 found two defects and nothing in the tool could act on them (G16). | Manual. |
| F8 | **A security model that extends.** Egress and secrets per role and per project, trust boundaries around agent-written text and files, and a rule for where tokens live when services exist. | Agent text could close unrelated issues from a PR body (#59); decision 36 (secret values never on the host) conflicts with a service that holds the Jev token. | Strong for sandboxes; nothing yet for services or multiple projects. |
| F9 | **Observable and auditable.** One place to see stage, status, disk and time per project, with the real gate output kept and no secrets in it. | An empty-looking gate log hid a full disk (G18). | `task status` per repo; logs vanish with the sandbox. |
| F10 | **Cost and usage records.** Tokens and dollars per run, per model, with a price table. | `Usage` flattens what the harnesses report (see `jev-questions.md` section 6). | Two sums and an optional dollar figure. |
| F11 | **Stable contracts.** Versioned run-record and decision-plane schemas, with compatibility rules. | Not yet needed; the cost of guessing early is high (we do not yet know the questions). | None. |
| F12 | **A trustworthy test and release base.** Flaky tests found and fixed, real-`sbx` tests, upgrade and install paths. | Two `task_*` tests failed under load and passed alone (G26). I saw no `.github` folder in the repo, so there is probably no CI. | Local gates only. |

## 5. A proposed order

**Principle (the user, 2026-10-03): optimize just in time, not prematurely.** Section 4 is a map of what the base will
need, not a work list. An item becomes work when something actually hits it (the gaps already hit are G16 to G26); the
rest waits, and the contracts in section 3 are not built until the questions they carry are known.

1. **Harden what exists (a "foundation 0" milestone):** F2, F5, F6 and F12 are already catalogued as gaps and are cheap
   compared with what depends on them. F3's disk check and F9's real gate output belong here too.
2. **Define the contracts and the project model:** F4 (namespaced ids, a project registry), F10/F11 (a richer `Usage`, the
   run-record schema, the price table, the generic external hook). This is design work: a planning interview and numbered
   decisions come first.
3. **Environments and placement:** F1 (templates and the strict mode) and F3 (a scheduler and a local-or-cloud backend).
4. **Services:** the evaluator and the router, once the questions are known (the scratch tests in `jev-questions.md`) and the
   contracts are settled.

## 6. Open decisions for the user

1. **Decision 36** (secret values are never readable on the host): keep it for services too, or let a service own its
   token in its own trust domain?
2. **Where do services run** at first: a command on the same machine (recommended), or hosted?
3. **Cloud:** is `sbx`'s own cloud sandbox the target, or something sbxm would schedule itself? What has to hold in the
   cloud that holds locally (egress rules, secrets, timeouts)?
4. **Is `sbxm task` part of the execution plane or its own workflow plane**, and is it still one repo per working
   directory?
5. **Order:** is step 1 (hardening) first, or does the project model (F4) have to come first because of the multi-project
   goal?
