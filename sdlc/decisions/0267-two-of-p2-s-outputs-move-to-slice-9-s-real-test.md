267. **Two of P2's outputs move to slice 9's real test (2026-10-24, the user's choice; narrows [262](b)):** the
capture run (`sdlc/spikes/m4-p2-capture.md`) saved every output and tried every point of P2 except two outputs it
couldn't produce: the not-logged-in message (it needs `sbx logout`, which signs the user out of both cloud OAuth
logins) and `rm`'s retryable "temporarily unavailable" error (it can't be triggered on demand). Until slice 9's
real-cloud test captures them, slice 3's removal retry is tested on the wording S14 recorded ("temporarily
unavailable; retry the delete"), slice 2's not-logged-in refusal on a fixture marked as made up (no run has seen the
real message), and slice 9 replaces both with real output where it gets it. With that, P2 is done. Why:
neither output can be had now without signing the user out or waiting on chance, and a wrong wording shows up at
slice 9, where it is cheap to fix.
