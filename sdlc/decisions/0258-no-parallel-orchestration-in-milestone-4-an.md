258. **No parallel orchestration in milestone 4; an experiment after the merge (2026-10-23, the user's answer;
builds on [245](g)):** milestone 4 adds no code for running cloud tasks in parallel. After the merge, an experiment
runs 2, then 3, cloud tasks at once as separate background `task run`s, and measures tasks per hour, cost and
failures. A `--parallel` option is built only if the experiment justifies it.
