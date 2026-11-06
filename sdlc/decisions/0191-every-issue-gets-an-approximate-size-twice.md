191. **Every issue gets an approximate size, twice (2026-10-13, the user's decision; a second narrow exception to
the skill freeze of [140] and [164], after [188]):** (a) **At creation**, by hand or from a review finding, an issue
gets a `size:S`, `size:M` or `size:L` label and a `**Size:**` line with the reason. The sizes are judged from what
the issue shows, not from hours: S is one module with no new state or file format and no open points; M is one
command or feature with tests and docs across a few files; L changes the state machine, `task.json` or the git trust
boundary, or has three or more open points, and is a candidate for splitting. (b) **At the "Decided" step**, when the
issue's open points are answered before a worker starts (the working rule of issue 206), the size is confirmed or
changed in the issue and on the label: an issue is at its vaguest when it is filed, and the scope is only fixed
then (issue 195 grew from its second answer). (c) **Where it lives:** the table is in the `sdlc-code-review` skill,
section 8, because that is the file a sandboxed reviewer is told to follow; `AGENTS.md` has the working rule and
points to it. (d) **Not yet done in code (issue 233):** the reviewer prompts spell out their own finding format with
no size line, and `task file-findings` leaves a `**Size:**` line out of the issue it files, so until then a filed
finding gets its size by hand. (e) **Left out on purpose:** checking sizes against the time and rounds `task.json`
records, until sizes prove unreliable. The open issues were sized on 2026-10-13 by label only; each gets its reason
line at its own "Decided" step.
