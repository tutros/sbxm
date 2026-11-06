241. **Jev's design for slice 14a (2026-10-22, the user answered `sdlc/jev.md` section 5; builds on [9], [18], [20],
[36], [99], [112], [120], [138], spikes S9 and S12):** the design in `sdlc/specs/jev-evaluator-14a.md` (the draft's
proposed decision "168", never written here) is the plan, with spike S12's corrections folded in, and three answers:
(a) **the LLM judge's ranking is the headline**; Jev's ranking is printed beside it, never blended, and Jev becomes the
headline only after a calibration run shows its verdicts match the user's on existing runs; (b) **one Jev call per
contestant per repeat**: S12 saw a Noul identical over five calls and a Score vary by about 0.03 on the 0-1 scale, so
there is no averaging inside a repeat, and `--repeat` is the way to reduce noise; (c) **`model` defaults to
`jev-latest`**, every result records the model that actually answered (the response's `model`) in `evals.json` and
`run.json`, `run show` prints it, and pinning (`model = "jev-1.13.0"`) is opt-in for a fixed comparison series. From PR 280's review (Q-1, Q-2, the
user chose the recommendations): (d) a task plus answer that is over the budget on its own makes **no call**; that
pair's Jev result is an `error` naming the oversized part, never a truncated answer; (e) in a chunked pair, every chunk
after the first sends the model the first response named, and a response naming another model makes the pair an
`error`, so one pair's score always comes from one model version.
