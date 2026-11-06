273. **External facts are checked first, and the real check runs early (2026-10-30, the third retro's proposal 2;
extends [244] and the no-open-questions rule):** before a worker starts on an issue that depends on an external
tool's behavior a spike recorded, the session re-runs the spike's key check by hand; after the worker, before the
review rounds, it runs the issue's real test (an ignored test or a by-hand smoke) and gives the result to the review.
An experiment for the next three issues that depend on an external tool; its measure is the problems found by real use
after the review rounds ended (the third retro's window: 2 on #330, and G67-G70 in milestone 4's end-to-end check).
