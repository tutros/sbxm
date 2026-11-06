246. **`run.json` records each pair's template (2026-10-23, the user's answers on issue 276, PR 269 review S-7; builds
on [128], [203], [206], [237]):** every entry of `run.json`'s `harnesses` gets a `template` key: the exact tag for a
(profile, harness) pair whose sandboxes were created from a template, `null` for an install-built pair. The key is
always written, never omitted, so a reader can tell "no template" from a run older than this field; a `run.json`
without the key (written before this decision) still loads. A rebuilt template keeps its config hash but freezes a
different agent version, so the tag is part of the run's provenance. README describes it in the `sbxm run` row.
