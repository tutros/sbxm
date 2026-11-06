257. **The task pipeline without a mount: same phases, bundles in and out (2026-10-23, the user's answer A for gates;
builds on [159], [160], [161]):** a cloud task runs the same phases as a local one. The repo goes in as a bundle by
`sbx --cloud cp`, the clone lives under `$HOME` in the sandbox, and the result comes out as a bundle that the host
verifies before fetching it into `repo.git` (S14 Q4). Host-tier gates are unchanged. There is one gate list for both
locations: this repo's `/tmp/target` becomes `$HOME/target`, the docs say to build under `$HOME` in the cloud, and
`doctor` warns when a cloud repo's gates name `/tmp`. Grounds: S14 Q5 found `/tmp` is a tmpfs of half the memory and a
cargo target folder filled it; one list keeps a repo's gates from drifting between locations.
