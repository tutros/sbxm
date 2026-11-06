199. **What goes in a template (2026-10-15, the user agreed with the recommendation):** by default a template holds
the tools only (S11's T0: the profile's install steps run once, in the build sandbox). With `--warm <repo>` it also
holds that repo's crate registry (`cargo fetch --locked`), the ONNX Runtime cache and the compiled dependencies from
one real build, with the repo's own build output removed (S11's T2, about 2.6 GB on the sandbox drive). The whole
build folder is never baked (5.2 GB, mostly rebuilt anyway). Why: only a warm template lets a sandbox run this repo's
gates with an empty allow list (the strict mode asked for on 2026-10-02), and it saves about 38 s on each sandbox's
first gate run; tools only keeps the faster start for a profile not tied to one repo. A warm template is tied to the
repo's `Cargo.lock`; what happens when the lock file changes is decided separately.
