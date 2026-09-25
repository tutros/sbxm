---
name: sdlc-planning
description: How to take a project or milestone from an idea to an implementation plan - interview the user in rounds of questions with a recommendation for each, log numbered decisions, verify unknowns with hands-on spikes, then write a milestone plan. Use when starting a new project, milestone or major feature, or when the user says "grill me" or asks what needs deciding.
---

# Planning phase

Planning turns a brief (e.g. `idea.md`) into three artifacts:

| Artifact | Contents |
|---|---|
| `decisions.md` | Numbered decisions, grouped by round with dates; spike results; open spikes |
| `milestone-N.md` | The implementation plan for one milestone |
| `CLAUDE.md` | The durable constraints that future sessions must know (points to the two files above) |

## Step 1: Read the brief

Read every existing file (brief, CLAUDE.md, READMEs). Don't design yet. Note what's stated, what's implied, and what's missing.

## Step 2: Interview in rounds

Ask questions in rounds of about 8–10. Order the rounds so each settles what the next depends on: *what is it* → *core mechanics* → *data and evaluation* → *config, security, lifecycle*.

Each question:
- is numbered, continuing across rounds (so answers like "18–19 agree" work)
- explains in one or two sentences why the decision matters
- ends with **→ My lean:** a concrete recommendation, so the user can just say "agree"

Also:
- Put the questions that block everything else first. Name the later topics briefly so the user sees what's coming.
- If the user uses a term you don't know (a product, acronym, tool), **ask**. Don't guess. If they point to docs or a plugin, read them and summarize what matters for the design.
- If an answer creates new questions, ask them in the next round before moving on.
- "Don't know yet" is a valid answer: turn it into a spike (step 4).

## Step 3: Log decisions right away

After each round, before asking the next, append to `decisions.md`:
- `## Round N (YYYY-MM-DD)` with one numbered entry per decision. Include the user's actual choice, not the lean, when they differ, plus any nuance they added.
- Facts learned from docs or research that affect the design, as a separate subsection, with source links.
- Never rewrite a past decision silently. When one changes, add a new numbered decision that says which earlier numbers it supersedes.

## Step 4: Verify unknowns with spikes

For each open question that depends on how an external tool or system actually behaves, define a spike (`S1`, `S2`, …) and run it before writing the plan when it affects the plan.

Spike rules:
- **Spec first when unattended:** if a spike runs in a subagent or background task, or the user won't watch each step, write `spikes/<id>.md` (task spec) and `spikes/<id>-results.md` (template) as defined in rule 5 of the `sdlc-implementation` skill, and get the user's approval before launching. The worker fills in only the results file; the main session checks the evidence and records the results in `decisions.md`.
- **Check reality, not docs alone.** Read `--help` and the specs, then test the behavior live.
- **Isolate:** throwaway resources with an obvious prefix (e.g. `sbxm-spike-*`), fake secret values scoped to the throwaway resource, temp work dirs. Change narrow things first, then widen.
- **When something fails, bisect:** remove one variable at a time until the cause is clear (e.g. "fails even without our kit → environment, not our config").
- **Clean up** everything the spike created, and confirm global state matches its starting point.
- **Record** results in `decisions.md` under "Spike results": what was tested, what was observed, and what it means for the design. Mark anything not verified as unverified.
- Report surprises as design consequences, and turn them into new numbered questions for the user when they change a decision.

## Step 5: Write the milestone plan

`milestone-N.md` contains:
1. **Goal:** one paragraph; what's in, what's deferred.
2. **Plan-level choices to confirm** (P1, P2, …) that the decisions don't cover yet, each with a recommendation. Ask before implementation starts, then record them as decisions.
3. **Structure:** modules/components and their responsibilities, in dependency order; dependencies.
4. **Interfaces:** config schemas (with examples), data mappings, command behavior tables, including edge cases and error behavior.
5. **Work order**, written as **vertical slices** (see the `sdlc-implementation` skill), each with how you know it's done.
6. **Manual end-to-end checklist** against the real system.
7. **Out of scope** and **risks** (with mitigations).

Reference decisions by number (`[27]`) instead of restating them.

## Step 6: Update CLAUDE.md

Put in `CLAUDE.md` only what every future session needs: the project's purpose, pointers to `decisions.md` and the current milestone plan, cross-cutting constraints, and gotchas discovered in spikes. No task lists, and nothing a reader could find in the code.

## Planning is done when

- every question that blocks the milestone has a numbered decision
- every spike that affects the milestone is run or explicitly deferred to a later milestone
- the plan's P-choices are confirmed
- CLAUDE.md points to the current plan

Then hand off to implementation.
