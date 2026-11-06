268. **A cloud create sbxm can't prove it made is left for the user (2026-10-27, the user's choice; refines [265]):**
a cloud sandbox is a task's own only when its `create` returned success and its lifetime entry was saved. A create
that was started but never recorded as returned (sbxm interrupted in between) is ambiguous: names are deterministic,
and no point-in-time check can tell this task's sandbox from a same-named one another caller made (PR 322 review,
three rounds). Every recovery and removal path leaves an ambiguous sandbox and its scoped secrets alone, refuses to
delete the task's record, and names the manual `sbx --cloud` commands; `--forget-ambiguous` on `task rm`, `task rm
--merged` and `task resume` drops the ambiguous entries without touching the cloud. The TTL deletes a forgotten one
eventually. Rejected: treating every `sbxm-task-*` name as sbxm's (it could delete another caller's sandbox) and a
random suffix per create (robust but changes sandbox naming across the milestone, size L). **Trigger to revisit:**
an ambiguous create happens in real use more than once.
