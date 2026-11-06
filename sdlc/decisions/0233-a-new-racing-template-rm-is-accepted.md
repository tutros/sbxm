233. **A `new` racing `template rm` is accepted (2026-10-20, milestone 3 slice 7 review Q-1, the user accepted the
risk; adds to [227]):** `template rm` checks for users, then removes. A sandbox creation that chose the tag and
created its sandbox but hasn't written `state.json` or `task.json` yet is invisible to that check, so `rm` can remove
the tag that sandbox was just created from. The window is seconds between two local commands the user starts by
hand, and `rm` is deliberate; no lock covers sandbox creation ([227]).
