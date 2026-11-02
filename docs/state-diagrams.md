# sbxm state diagrams

Drawn from the code on `main` (a8ee1f1), 2026-11-02. Sources: `src/cli.rs`, `src/task/machine.rs` (the
task state table), `src/task/record.rs`, `src/task/pipeline.rs`, `src/commands/*.rs`. `sbxm task states`
prints the task table itself.

## 1. Invocations

```mermaid
flowchart LR
  sbxm([sbxm]) --> L[Sandbox lifecycle<br/>new · open · list · stop · rm]
  sbxm --> C[config init · show · profiles-dir]
  sbxm --> D[doctor · status --watch]
  sbxm --> T[template build · ls · rm]
  sbxm --> R[run &lt;config&gt; · run init · run show]
  sbxm --> K[task ...]
  K --> K1[init]
  K --> K2[start --issue/--spec]
  K --> K3[review --issue/--pr/--spec]
  K --> K4[run = start + review<br/>one or many targets]
  K --> K5[gates · resume]
  K --> K6[finish]
  K --> K7[rm --issue/--pr/--spec/--merged]
  K --> K8[status · logs · states · file-findings]
```

Read-only (no state change): `list`, `config show`, `config profiles-dir`, `doctor`, `status`,
`template ls`, `run show`, `task status`, `task logs`, `task states`.

## 2. Sandbox lifecycle (`new`, `open`, `stop`, `rm`)

```mermaid
stateDiagram-v2
  [*] --> Checks: new / open (no sandbox)
  Checks --> Refused: bad name, exists, low disk,<br/>missing secret, strict template refusal
  Checks --> KitBuilt: template::choose → tag or install kit
  KitBuilt --> Refused: validate_kit fails
  KitBuilt --> Running: sbx create, state.json written
  Running --> Attached: open (hash matches)
  Attached --> Running: detach
  Running --> Drifted: config hash changed
  Drifted --> Refused: open without --rebuild
  Drifted --> KitBuilt: open --rebuild (old removed only after new kit validates)
  Running --> Stopped: stop
  Stopped --> Attached: open
  Running --> Removed: rm
  Stopped --> Removed: rm
  Removed --> Purged: rm --purge + confirm (workspace deleted)
  Refused --> [*]
  Removed --> [*]
  Purged --> [*]
```

## 3. Task record: stage × status (`src/task/machine.rs`)

Every stage carries a status: `prepared` {running, failed}; `working`/`fixing` {running, completed,
timed-out, failed}; `gating` {running, passed, gates-failed}; `reviewing` {running, completed, failed};
`ready`/`finished` {ok}. "Interrupted" = `running` whose recorded process (pid + start time) is gone.

```mermaid
stateDiagram-v2
  [*] --> Prepared: task start (issue/spec) · task review --pr (PR)
  Prepared --> Working: worker runs (issue, spec)
  Prepared --> Gating: PR task
  Prepared --> Reviewing: PR task, no gates

  Working --> Gating: completed / timed-out
  Fixing --> Gating: completed / timed-out

  Gating --> Reviewing: passed
  Gating --> Fixing: gates-failed + rounds left
  Gating --> GateStop: gates-failed, no rounds left<br/>or gate could not run

  Reviewing --> Fixing: issue/spec: must-fix > 0, rounds left, no repeat
  Reviewing --> Reviewing: issue/spec: narrow review clean → one full review
  Reviewing --> Ready: issue/spec: full review clean<br/>PR task: any finding count (no fix rounds)
  Reviewing --> ReadyStopped: issue/spec: rounds-exhausted / repeat-finding

  Ready --> Finished: issue/spec: task finish
  ReadyStopped --> Finished: issue: task finish (draft PR)<br/>spec: local sink, or push with --push-unresolved
  ReadyStopped --> Fixing: issue/spec: task resume [--rounds N]
  Ready --> Gating: PR task: review --pr on a new head

  state "Ready (ok)" as Ready
  state "Ready, stopped set" as ReadyStopped
  state "Gating: gates-failed (stops)" as GateStop
  Finished --> [*]
```

Failure and resume (issue/spec tasks; `task resume`):

```mermaid
stateDiagram-v2
  direction LR
  state "prepared failed / interrupted" as P
  state "working failed / interrupted" as W
  state "gating ended / interrupted" as G
  state "reviewing failed" as RVF
  state "reviewing interrupted" as RVI
  state "reviewing completed (process gone)" as RC
  state "fixing failed / interrupted" as F
  P --> Working: resume (sandbox made again)
  W --> Working: resume (same clone)
  G --> Reviewing: resume / review (gates re-run if failed)
  RVF --> Reviewing: resume / review (retry)
  RVI --> Reviewing: resume only (review refuses it)
  RC --> Fixing: resume replays recorded review<br/>(fix round, else ready)
  F --> Fixing: resume (same input)
```

`task gates` re-runs gates from `working`/`fixing` done or any ended `gating`. `task rm` is allowed from
any state that is not running (or running but interrupted). Issue and spec tasks have no dead ends
(`tests/task_machine.rs`). A PR task has four, where only `rm` then `review --pr` again moves on:
prepared/failed, gating interrupted, reviewing interrupted, reviewing completed (issue #379).

## 4. `task run` / `task review` processing (issue or spec)

```mermaid
flowchart TD
  A([task run --issue N / --spec F]) --> PF{Preflight, before any write:<br/>sbx answers, secrets, disk,<br/>repo, eligibility, cloud checks}
  PF -- refused --> X1([exit: refusal, nothing written])
  PF --> PR[prepare: task folder, repo.git,<br/>clone, worker sandbox<br/>local or cloud with TTL]
  PR -- fails --> PFAIL([prepared/failed → resume or rm])
  PR --> WK[worker: headless harness<br/>under in-sandbox timeout]
  WK -- failed --> WFAIL([working/failed → resume])
  WK -- completed / timed-out --> COL[bundle + fetch commits<br/>onto agent ref]
  COL --> GT{gates<br/>sandbox tier, host tier}
  GT -- could not run --> GSTOP([stop after start: gates-failed])
  GT -- failed, rounds left --> FX[fix round<br/>round += 1]
  GT -- failed, no rounds --> GSTOP2([stop: gates-failed])
  FX --> GT
  GT -- passed --> RVW[reviewer sandbox<br/>review-n.md: Must-fix, Risk]
  RVW -- reviewer failed --> RFAIL([reviewing/failed → resume or review])
  RVW --> MF{must-fix?}
  MF -- 0, narrow --> RVW
  MF -- 0, full --> RDY([ready ok])
  MF -- >0, Repeat of --> ST1([ready, stopped: repeat-finding])
  MF -- >0, budget used --> ST2([ready, stopped: rounds-exhausted])
  MF -- >0, rounds left --> FX2[fix round] --> GT
```

A cloud task's worker sandbox and its scoped secrets are removed whenever the task stops. A list
`task run` runs targets in order; only `sbx` not answering, a missing secret, low disk or `exec`
itself failing stops the whole list (decision 193).

## 5. Completion: `task finish` and `task rm`

```mermaid
flowchart TD
  R([ready ok]) --> FN{task finish}
  FN -- issue task --> PUSH[rewrite commits defanged,<br/>push branch] --> PRQ{must-fix left?}
  PRQ -- no --> PROPEN([PR opened, risk section first → finished])
  PRQ -- yes --> PRDRAFT([draft PR with Unresolved → finished])
  FN -- spec, sink local --> LOC([branch kept in repo.git,<br/>fetch hint printed → finished])
  FN -- spec, sink push --> SPQ{findings left?}
  SPQ -- no --> SP([branch pushed to origin, no PR → finished])
  SPQ -- yes --> SREF([refused unless --push-unresolved])
  FN -- PR task --> PRREF([refused: a PR task has no finish])
  ANY([any non-running task]) --> RM{task rm}
  RM -- unfetched spec result --> RMREF([refused])
  RM --> GONE([sandboxes, secrets, folders removed])
  MERGED([task rm --merged]) --> ARCH[archive text records to<br/>.sbxm/task-archive/id] --> GONE
```

## 6. Comparison run (`sbxm run <config>`)

```mermaid
flowchart TD
  S([sbxm run config]) --> V{RunConfig::load<br/>+ preflight: seed, profiles,<br/>secrets, cosine model, disk}
  V -- error --> E1([exit 1, nothing written])
  V --> ID[reserve run id, write run.json<br/>+ run-config.toml, build kits]
  ID --> WAVE[repeat k: pairs in parallel]
  WAVE --> PAIR[per pair: create sandbox, seed,<br/>headless run]
  PAIR --> OUT{Completed / TimedOut / Failed}
  OUT --> DIFF[diff, checks, remove sandbox,<br/>save answer/diff/transcript/result.json]
  DIFF --> MORE{more repeats?}
  MORE -- yes --> WAVE
  MORE -- no --> J[judge per repeat, blind labels]
  J --> CS[cosine per repeat, if configured]
  CS --> JV[Jev per repeat, if configured]
  JV --> DONE[mark completed_at, rank, print]
  DONE --> EX{any contestant could not run?}
  EX -- yes --> E2([exit 1])
  EX -- no --> E0([exit 0])
```

Timed-out pairs keep their partial output and are still evaluated.

## 7. Templates (`template build`)

```mermaid
stateDiagram-v2
  [*] --> Checks: template build (claude, codex, pi; sbx >= 0.46.0)
  Checks --> Refused: unsupported harness, old sbx, build lock held
  Checks --> Building: baked kit, optional --warm
  Building --> Recorded: tag saved, record JSON written
  Recorded --> Recorded: previous tag cleaned up
  Recorded --> Used: new/open/task/run choose the tag
  Recorded --> Removed: template rm (refused while users record it)
  Refused --> [*]
  Removed --> [*]
```
