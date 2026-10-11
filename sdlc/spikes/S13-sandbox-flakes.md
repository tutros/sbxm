# Spike S13: do the flaky tests vanish in a Linux sandbox? (issue #111)

Run 2026-10-11 on commit `5b8a9a85d3280600fb0182f32e1e2c0de2d8ffca` (branch `spec-spike-111-7da7e4`, head of
`main` at the time), in a Linux sandbox: Ubuntu 26.04.1 LTS, 4 CPUs, 7.8Gi RAM, `/tmp` on the `overlay` mount
with 18G free at the start (20G total, 7% used) and 7.7G free at the end (59% used). Question (from issue
#111, following up on issue #104): issue #104 lists three tests that each failed once in a full parallel
`cargo test` and passed on every re-run, with the known Windows cause being a short-lived file lock (`git`
`Permission denied` when it replaces a file another process still holds, which Linux allows). Do the three
flakes reproduce in a Linux sandbox, where that specific lock cannot occur?

**Verdict: clean sandbox runs. 25/25 full-suite runs passed (20 unloaded + 5 more under concurrent load);
none of the three tracked flakes, or any other test, failed.** This is consistent with "the Windows file
lock is the main cause", but 25 clean runs is a small sample against a flake that showed once across
however many runs preceded issue #104 — see "What this does and doesn't show" below.

## Setup used

```
CARGO_PROFILE_DEV_DEBUG=0 CARGO_TARGET_DIR=/tmp/target cargo test --no-run
```

Build once (74s), then the full suite run repeatedly with:

```
CARGO_PROFILE_DEV_DEBUG=0 CARGO_TARGET_DIR=/tmp/target cargo test --no-fail-fast
```

20 such runs, one after another, each run's full stdout+stderr captured to its own log file under
`/tmp/spike-111/run-NN.log` (not in the repo), with exit code and wall-clock duration recorded to
`/tmp/spike-111/results.csv` right after each run. All 20 passed, so a further 5 runs were done under
load: the same command, run 5 more times (counted), while a second, independently-looping
`CARGO_TARGET_DIR=/tmp/target2 cargo test --no-fail-fast` ran concurrently in the background on the same
4 CPUs (started before run 21, stopped after run 25; only the first/counted command's result is reported
for runs 21-25, per the task's instructions).

## Results

| run | exit code | duration | failed tests | group |
|---|---|---|---|---|
| 1 | 0 | 52s | none | unloaded (1st run includes warm-up/caching) |
| 2 | 0 | 42s | none | unloaded |
| 3 | 0 | 38s | none | unloaded |
| 4 | 0 | 38s | none | unloaded |
| 5 | 0 | 39s | none | unloaded |
| 6 | 0 | 38s | none | unloaded |
| 7 | 0 | 39s | none | unloaded |
| 8 | 0 | 40s | none | unloaded |
| 9 | 0 | 39s | none | unloaded |
| 10 | 0 | 38s | none | unloaded |
| 11 | 0 | 38s | none | unloaded |
| 12 | 0 | 38s | none | unloaded |
| 13 | 0 | 37s | none | unloaded |
| 14 | 0 | 39s | none | unloaded |
| 15 | 0 | 39s | none | unloaded |
| 16 | 0 | 39s | none | unloaded |
| 17 | 0 | 39s | none | unloaded |
| 18 | 0 | 38s | none | unloaded |
| 19 | 0 | 38s | none | unloaded |
| 20 | 0 | 47s | none | unloaded |
| 21 | 0 | 69s | none | under load (separate group) |
| 22 | 0 | 66s | none | under load (separate group) |
| 23 | 0 | 55s | none | under load (separate group) |
| 24 | 0 | 56s | none | under load (separate group) |
| 25 | 0 | 75s | none | under load (separate group) |

All 20 unloaded runs passed (exit 0), so step 5 of the spike (5 more runs under load) was done too. The 5
loaded runs are a separate group from the 20 (they ran concurrently with a second, independent full test
run contending for the same 4 CPUs, which is why their durations are noticeably longer); only their own
exit code counts.

Every run's 108 `test result:` summary lines (one per test binary) read `ok` with `0 failed`, in every one
of the 25 logs (spot-checked across runs 1, 20 and 25, and confirmed for all 25 by `grep`), and across the
whole suite in run 1 (taken as representative) 1,482 individual tests passed. No log contains the strings
`FAILED`, `panicked` or a line starting `error` (`grep -lE 'FAILED|panicked|^error' /tmp/spike-111/run-*.log`
matched zero files across all 25 logs, unloaded and loaded alike).

## Distinct failures

None. Zero of the 25 runs failed, so there is nothing to report for issue #104's three tracked tests
(`tests/task_start.rs::each_pick_gets_its_own_task_sandbox_and_collected_commits`,
`tests/run_results.rs::result_json_records_status_usage_and_identity`,
`tests/task_review_flow.rs::a_clean_review_runs_the_gates_first_and_ends_ready_without_a_fix_round`) or any
other test.

## Resource notes

- CPUs: 4 (`nproc`).
- Memory: 7.8Gi total, free at the start of the spike.
- `/tmp` (on the `overlay` root mount): 18G free / 20G total (7% used) before run 1; 7.7G free (59% used)
  after run 25 and cleanup of the load target. No run hit a disk-space or memory error.

## What this does and doesn't show

**Supports:** "clean sandbox runs: the Windows file lock is the main cause" — all three of issue #104's
flaky tests, which were reported on Windows (two of them) or in a sandbox (one, `run_results.rs`), passed
every one of the 25 times they ran here, including 5 runs under added CPU contention (which is the
kind of timing pressure most likely to surface a race).

**Does not show:**
- This result says nothing about the product code path on a **Windows host**: the file-lock cause named
  in issue #104 is specific to Windows's file-replace semantics, which this Linux sandbox cannot exhibit
  either way, so a clean run here does not independently confirm that diagnosis — it is only consistent
  with it (the absence of the one known Windows-only trigger, here, found no failure).
- 25 clean runs do not prove a rare flake is gone. If a flake's true per-run failure probability were as
  high as 1 in 25 (p = 0.04), the chance of 25 consecutive clean runs is (1-0.04)^25 ≈ 36%, so this result
  would not be surprising even if the flake still existed at that rate. Treating "zero failures in 25
  independent Bernoulli trials" as evidence with the standard "rule of three" bound, a 95% upper bound on
  the true failure rate is about 3/25 ≈ 12% per run; put the other way, this run only rules out, at 95%
  confidence, a per-run failure rate above roughly 1 in 8. The three flakes in issue #104 each occurred
  once, with no stated denominator of how many runs preceded that one failure, so there is no way to
  compare this spike's implied rate ceiling against their true (unknown) rate; a failure as rare as "one
  in a few hundred runs" is entirely compatible with seeing zero failures here.
- The `run_results.rs` failure in #104 was explicitly noted as possibly having "another cause" (it
  happened inside a sandbox already, not on Windows), and this spike's clean runs don't rule that other
  cause out either — they just didn't trigger it this time.

## Exact commands (to repeat the experiment)

```bash
mkdir -p /tmp/spike-111

# Build once
CARGO_PROFILE_DEV_DEBUG=0 CARGO_TARGET_DIR=/tmp/target cargo test --no-run

# 20 unloaded runs, one after another (loop started once)
for i in $(seq 1 20); do
  LOG=$(printf '/tmp/spike-111/run-%02d.log' "$i")
  START=$(date +%s)
  CARGO_PROFILE_DEV_DEBUG=0 CARGO_TARGET_DIR=/tmp/target cargo test --no-fail-fast > "$LOG" 2>&1
  EXIT=$?
  END=$(date +%s)
  echo "$i,$EXIT,$((END - START))" >> /tmp/spike-111/results.csv
done

# Resource notes
df -h /tmp | tail -1
nproc
free -h

# 5 more runs under load (only the first command's result counts)
( while [ ! -f /tmp/spike-111/load-stop ]; do
    CARGO_PROFILE_DEV_DEBUG=0 CARGO_TARGET_DIR=/tmp/target2 cargo test --no-fail-fast \
      > /tmp/spike-111/load-run-$RANDOM.log 2>&1
  done ) &
for i in $(seq 21 25); do
  LOG=$(printf '/tmp/spike-111/run-%02d.log' "$i")
  START=$(date +%s)
  CARGO_PROFILE_DEV_DEBUG=0 CARGO_TARGET_DIR=/tmp/target cargo test --no-fail-fast > "$LOG" 2>&1
  EXIT=$?
  END=$(date +%s)
  echo "$i,$EXIT,$((END - START))" >> /tmp/spike-111/results.csv
done
touch /tmp/spike-111/load-stop
```
