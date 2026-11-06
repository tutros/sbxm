238. **A lock file's line endings don't count (2026-10-21, milestone 3 slice 12 end-to-end step 5, the user agreed
with the recommendation; changes [207], [216]):** the lock hash reads every CRLF as LF, whether the file comes from a
folder or a commit. The end-to-end check found that a template warmed from a Windows checkout with `core.autocrlf`
(CRLF files) never matched a task, which reads the commit's LF blob, so a strict profile refused every task. A lock
file that differs only in its line endings parses the same, so the cached dependencies still fit.
