264. **A strict cloud sandbox denies every host outside a fixed provider list, and says which (2026-10-24, the user's
answers to milestone 4's P-choices; P9 (a) with (i), to match [221], [232]; builds on [255]):** the runtime list is a
fixed provider host list per harness in code plus the profile's `[template].allow`; every other host gets a
per-sandbox `policy deny`. This applies to strict worker, reviewer and fix-round sandboxes alike. The user added:
sbxm is *removing* hosts, so when something fails unexpectedly in a strict cloud sandbox, the first check is whether
a denied host is actually needed. So each sandbox's denied hosts are written to the task's `run.log`, the docs slice
adds a troubleshooting line for this check, and the plan's Risks section names it.
