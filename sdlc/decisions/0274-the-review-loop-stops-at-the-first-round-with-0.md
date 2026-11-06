274. **The review loop stops at the first round with 0 must-fix (2026-10-30, the third retro's proposal 3; narrows
[251]'s re-review):** that round's should-fix items are fixed by hand and re-reviewed narrowly, on the fix's diff
only; anything the narrow re-review finds outside that diff is filed as an issue with a trigger, not fixed in another
round. An experiment for the next three PRs that reach a 0-must-fix round with should-fix items; its measure is the
review rounds after the first 0-must-fix round (the third retro's window: 5, #322 three and #329 two).
