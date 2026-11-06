263. **A cloud worker sandbox lives through the task's automatic run (2026-10-24, the user's answers to milestone 4's
P-choices; P5, a new option (d); builds on [118], [259]):** the cloud worker sandbox lives through the task's
automatic run (worker, sandbox gates, review rounds, fix rounds) and is removed when the task stops (ready, or
stopped for any reason). `task resume` rebuilds it from the recorded profile and a bundle of the task's branch when
needed, as it rebuilds a gone worker sandbox today [118]. Its TTL is the worker's time limit + every sandbox gate's
timeout + `fix_rounds` x (the reviewer's time limit + the worker's time limit + every sandbox gate's timeout) + 30
minutes. Reviewer sandboxes stay one per round, each with a TTL of the reviewer's time limit + 30 minutes. This
gives [259]'s "its phase's time limit" its meaning for the worker sandbox. Grounds: fix rounds keep a warm build (a
cold build is 233 s on the large shape, S14 Q5), and nothing is billed while a task waits for the user (up to 2h25m
from open to merge in the loop trial).
