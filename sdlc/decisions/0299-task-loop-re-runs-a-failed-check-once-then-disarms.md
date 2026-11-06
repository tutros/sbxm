299. **`task loop` re-runs a failed check once, then disarms its PR or pauses arming on a red `main` (2026-11-05, the
user's answers on issue #442; uses [280], [293], [297], [298]):** a failed check waited until someone looked (PR #440
was armed when its Windows job failed; `main` after PR #437 stayed red the same way), and [297] watches CI only while
a session is open. The fix of a red PR by itself is #448, not this. (1) On each pass, after the resolves of [298] and
before the wait on 5 PRs, the loop reads the workflow runs (`gh api repos/<repo>/actions/runs?head_sha=<sha>`, every
workflow, CodeQL included) of the head of every open PR it opened (drafts too; a PR it didn't open is not read), of
the merge commit of every PR it armed in this run that merged since the last pass (decision 280's case), and of
`main`'s tip whoever merged it. After a restart only `main`'s tip and the open loop PRs are read. (2) A run still
queued or in progress: nothing this pass; a commit of `main` (a merge commit, or a tip after `main` moved on) whose
runs aren't over, or that has no runs listed yet (GitHub lists them some time after the push), is read again next
pass, in memory only; with no run listed 1 hour after the loop first watched it, it says so once ("no CI runs seen
for <commit> after 1 h; still watching") and goes on watching. A failed run
(`failure`, `timed_out`, `startup_failure`; `cancelled` is not a failure) at attempt 1 gets its failed jobs re-run
once (`gh run rerun <id> --failed`); a pass after the re-run ends it (a runner fault isn't a break). GitHub's attempt
number is the only state: `attempt` > 1 failed is the second failure, so a restart re-runs nothing twice. (3) A second
failure on a loop PR: auto-merge disarmed if armed, the PR awaits the user, said once with a row in the loop's log.
(4) A second failure on `main`: the pause, `<base>/.sbxm/loop-pause.json` (the commit, the run's id, URL and workflow,
who merged it, when), shown in the Loop section of `sbxm status`. While it is there the loop goes on running tasks and
opening PRs but arms nothing (neither [293]'s arm nor [298]'s re-arm), and disarms auto-merge on every loop PR that
has it armed. Only the user lifts it, with `sbxm task loop --clear-pause` (it says what it removed, or that there was
none, and runs nothing); the loop never does, not even when `main` turns green ([280]). A pause file that can't be
read counts as a pause, with a warning naming it. A pause the user cleared isn't set again by the same loop for the
same run, but another run's second failure on it gets its own pause; a restarted loop that still finds it red sets
it again. A pause that can't be written stops the loop.
Polling inside the loop's pass for now; a push-based event bus is the direction once more than one machine or several
cloud workers run at once, or polling delay costs real time (the user, 2026-11-06).
