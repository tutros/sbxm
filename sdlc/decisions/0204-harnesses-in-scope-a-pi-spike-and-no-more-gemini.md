204. **Harnesses in scope, a Pi spike, and no more Gemini work (2026-10-15, the user's decision):** templates cover
`claude` and `codex` in this milestone; `codex` gets a real-`sbx` test with `-t` as the first slice (S11 tested only
`claude`). Spike **S15** checks whether templates work with `pi`, whose `agent_arg` is a kit of its own [73]: the
user expects to use sbxm with Pi more once the main features are built, so Pi joins the milestone if S15 passes.
`antigravity` and `gemini` are refused with "templates are not supported for this harness yet". **Gemini is dropped
from future work** (the user, 2026-10-15; upstream deprecated Gemini CLI and the built-in agent had no working Google
network permissions here, [103]): no new feature supports it. Whether `--harness gemini` is removed from the code is
a separate issue, the one [103] left unfiled.
