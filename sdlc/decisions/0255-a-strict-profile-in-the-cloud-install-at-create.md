255. **A strict profile in the cloud: install at create, then remove the build hosts (2026-10-23, the user's answer
B, confirmed by spike S17; builds on [200], [221], [232]):** a strict profile's cloud sandbox is created with the
build allow list so its install steps can run. Before the worker starts, sbxm removes every host
outside the runtime list from the sandbox's cloud policy and checks that `policy ls` shows only the runtime list,
like decision 232 does for a strict `codex` sandbox. In the cloud this replaces [200](a)'s "no install steps" (there
is no template to start from, [254]); [200](b), only the runtime list while the agent runs, and [200](c), never a
quiet fallback, still hold. The fallback the user named, A (refuse strict profiles in the cloud until cloud
templates, a non-strict cloud profile instead), was to apply if rules couldn't be removed from a running cloud
sandbox. Spike S17 (2026-10-23, `sbx` v0.46.0) showed they can: `policy rm network --sandbox <name> --resource
<host> -f` and `policy deny network --sandbox <name> <host>` both exit 0, print the resulting list and take effect
at once on the running sandbox, for a kit's host and for a host of the agent's own built-in list alike. So B holds
and A is not needed.
