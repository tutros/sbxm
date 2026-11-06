247. **How spike S14 runs (2026-10-23, the user's five answers):** (a) Claude runs it hands-on in the main session
between loop tasks, since it needs the host's `sbx --cloud` and the cloud's secrets (no spec for a worker). (b) A
hard cap of $10; every spike sandbox gets `--ttl 1h` and is removed after its question; the default shape except one
large shape for the build timing; the user reads the bill before and after, and Claude stops and asks before the cap.
(c) Secrets: a fake value scoped to one sandbox first, then the user's real Claude and OpenAI logins through
`sbx secret set <provider> --oauth --cloud` (the user signs in), stored only in the cloud and removed at the end unless
a cloud milestone follows. (d) Q1-Q4 (egress, secrets, timeout, bundle in and out) must pass or there is no milestone;
Q7 (Codex in the cloud, added by the user) decides whether reviews move too; Q5 (time and cost) and Q6 (templates)
inform the plan only. (e) Order: Q1, Q3, Q4, Q2, Q7, Q6, Q5; a failing must-pass question is narrowed down once, then
the spike stops. Question 7 was added to the issue on 2026-10-23.
