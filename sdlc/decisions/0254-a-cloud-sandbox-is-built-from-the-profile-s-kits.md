254. **A cloud sandbox is built from the profile's kits; cloud templates come later (2026-10-23, the user's answer;
builds on [62], [199]-[206], [248]):** a cloud sandbox gets the profile's two kits at `sbx --cloud create` (S14 Q1),
and the install steps run at create. Cloud templates are deferred; the trigger is cloud setup taking over about 2
minutes a task. Grounds: S14 Q6 showed a cloud template can't be combined with a kit and has its size baked in, so a
template design needs its own work, while kits work today (24 s to ready for `sbxm-dev`'s install steps on the large
shape, S14 Q5).
