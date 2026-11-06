277. **Human review of a comparison run: a checked file, CLI only (2026-10-31, the user's answers on issue #347;
builds on [18], [21], [22], [100]):** `sbxm run review <run-id>` lays out each repeat under blind A/B/C labels of its
own (a shuffle with a seed other than the judge's, since `run` and `run show` already reveal the judge's mapping):
each pair's answer and diff, and a `scores.toml` asking for every rubric criterion. `--submit` checks the file
(`pass_fail` true or false, a `scale` value one of its levels), saves the scores in `evals.json` under `human` and
prints a third ranking with the real contestant names. One reviewer; a second `--submit` overwrites; a blank
criterion is unscored, never 0; a file with no scores is refused. A local web page stays later; it would read the
same saved scores.
