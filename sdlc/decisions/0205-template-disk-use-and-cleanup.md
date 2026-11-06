205. **Template disk use and cleanup (2026-10-15, the user agreed with the recommendation):** (a) `template build`
removes the previous template of the same profile and harness once the new one is saved, unless one of sbxm's
sandboxes or tasks still records it, in which case it stays and the build says so; (b) `template rm` refuses while
a template is in use and names the sandboxes and tasks using it (from `state.json` and every `task.json`), because
`sbx template rm --force` does not refuse (S11, question 8); (c) `doctor` lists each template with its size and age
and the total, beside the free-space line of [194]; (d) nothing else removes a template: `rm`, `task rm` and `task
rm --merged` only stop recording it as in use. Why: 1.3 to 2.6 GB per template on the drive that already ran out,
and a rebuild makes a new tag [203].
