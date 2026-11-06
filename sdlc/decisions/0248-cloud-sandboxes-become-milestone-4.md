248. **Cloud sandboxes become milestone 4 (2026-10-23, the user's decision after spike S14; builds on [56], [159],
[161], [199]-[239]):** the spike passed every must-pass question and Q7 (`sdlc/spikes/S14.md`, $0.79 spent in total), so
running sbxm's sandboxes on Docker Cloud Sandboxes is planned as its own milestone, through a planning interview like
the templates milestone's (issue 211), tracked by issue 310. Its inputs are the spike's design facts: a template
can't be combined with a kit and has its size baked in, so a cloud template sandbox is configured by `create` flags;
`exec` can't run as root, so root setup happens only through a kit at create; secrets attach at create, are scoped by
sandbox name, take literal values and set a header; global cloud secrets reach every cloud sandbox, so each one is
scoped to its sandbox; egress is refused at DNS, by connection refused or by a proxy 403; a timeout with an orphaned child ends in a stream
error rather than 137; `/tmp` is a tmpfs of half the memory; removal can need a retry; there is no workspace mount, so
code goes in and out by bundle. Until the planning starts, the user's two cloud OAuth secrets and the spike's template
(`sbxm-spike-s14-rust`) stay in the cloud.
