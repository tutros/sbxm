290. **A strict template made stale by `main`'s lock files is rebuilt from `main`; a branch that changes them stops
(2026-11-03, the user's answers on issue #402; refines [201] and [203](b), replaces [235]'s hint):** when a strict
profile's warm template (built for the profile as it is now, its image still in `sbx`) has other lock files than the
task's base branch, the task's sandbox is not refused: sbxm fetches `main` (origin's default branch, whatever base the task started from) into its host-owned
`repo.git` (as branch `sbxm-template-base`, apart from the task's own base ref), and when the template's lock files
differ from that tip too, checks it out cleanly (as `task::repo` makes checkouts, never an agent's clone), runs
`template build --warm` from it, removes the checkout and goes on, printing one line with the new tag. A task whose
own base is older than `main` now (a resumed one) never rebuilds the template back to its older lock files. Lazily, at the first task sandbox (worker, remade worker
or reviewer) that needs the template; no merge watcher. Two tasks needing the same rebuild make one build: the second
waits on the build lock of [212] instead of being refused, and uses the template the first built when it already holds
the base's lock files. A rebuild that fails stops the task, naming `sbxm template build <profile> --warm <a clean
checkout of main>`. Line-ending-only differences rebuild nothing [238]. Then, when the commit the sandbox checks out (a
task's branch, a PR's head) has lock files other than the task's own base, the task stops with the hand-back of [291] (after the
rebuild, when `main` changed too). Warming from agent-written code (a worker's tip, an unmerged PR head) is never
automatic: a warm build runs `cargo` and its build scripts with network, and the result becomes every later sandbox's
template. So [235]'s advice (set `strict = false`, or warm a template from the task's tip) is dropped; an issue or spec
task's stop still names `task resume`. Non-strict profiles keep [203](b)'s warning and installs.
