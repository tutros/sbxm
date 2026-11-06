250. **An issue body is read back before it's labeled (2026-10-23, the second retro's proposal 2; amends [245](b)'s
labeling step):** before adding `ready-for-worker`, the issue is read back with `gh issue view` and checked for three
things: no `PR: #n` line names a merged or closed PR, the body kept its line breaks (bodies are written with the Write
tool and passed with `--body-file`), and every recorded answer agrees with the decisions it touches. An experiment for
the next 15 labeled issues; its measure is how many need a body fix or are refused before any write (the first window:
3 of 15, #222, #272, #273).
