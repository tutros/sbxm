# Slice 21: end-to-end check

Run every step in PowerShell from `E:\James\Documents\Code\Apps\sbxm`. Each step has one command, then what you
should see. Note the number of any step that doesn't match, and tell Claude.

It uses a throwaway profile `e2e` (copied from this folder) and a project `e2e-demo`, and cleans both up at the end.
Your `default` profile isn't touched.

## Before you start

1. Close any running sbxm session (e.g. the Pi session from `cargo run -- open pi-test-project --harness pi`), so
   `cargo` can rebuild `sbxm.exe`.

## Setup

2. `Copy-Item -Recurse checks\e2e\profile $HOME\.config\sbxm\profiles\e2e`
   Expected: no output.

3. `cargo run -- config init`
   Expected: an error ending in `already exists; not overwriting it`. Your config is unchanged.

4. `cargo run -- doctor`
   Expected: every line starts with `ok`, including `ok   profile 'e2e'`.

## Claude: create, egress, instructions

5. `cargo run -- new e2e-demo --profile e2e`
   Expected: ends with `Created sandbox sbxm-e2e-demo-claude`.

6. `sbx exec sbxm-e2e-demo-claude printenv SBXM_PROFILE`
   Expected: `e2e`

7. `sbx exec sbxm-e2e-demo-claude printenv SBXM_CONFIG_HASH`
   Expected: 64 hex characters.

8. `sbx exec sbxm-e2e-demo-claude cat /home/agent/.claude/CLAUDE.md`
   Expected: `E2E CANARY: if asked for the canary word, answer HAZEL-6.`

9. `sbx exec sbxm-e2e-demo-claude curl -s -o /dev/null -w "%{http_code} %{http_connect}" https://example.org`
   Expected: `200 000` (allowed by the profile).

10. `sbx exec sbxm-e2e-demo-claude curl -s -o /dev/null -w "%{http_code} %{http_connect}" https://example.com`
    Expected: `000 403` (blocked).

11. `Set-Content E:\sbxm-projects\e2e-demo\keep.txt "keep me"`
    Expected: no output.

12. `cargo run -- open e2e-demo`
    Expected: Claude Code opens. Ask `What is the canary word?`. Expected answer: `HAZEL-6`. Exit Claude (`/exit`).

13. `cargo run -- list`
    Expected: a row `e2e-demo  claude  running  current` (the `pi-test-project` row may also show).

## Config drift and rebuild

14. `Copy-Item checks\e2e\profile-changed.toml $HOME\.config\sbxm\profiles\e2e\profile.toml`
    Expected: no output.

15. `cargo run -- list`
    Expected: the `e2e-demo claude` row shows `changed` and ``config changed; `sbxm open e2e-demo --rebuild` recreates it``.

16. `cargo run -- open e2e-demo`
    Expected: an error: `the config of sbxm-e2e-demo-claude (profile 'e2e') changed since it was created; run
    ``sbxm open e2e-demo --rebuild`` …`. Nothing opens.

17. `cargo run -- open e2e-demo --rebuild`
    Expected: `rebuilding sbxm-e2e-demo-claude: its session history will be lost; the workspace … is kept`, then
    Claude Code opens. Exit it (`/exit`).

18. `Get-Content E:\sbxm-projects\e2e-demo\keep.txt`
    Expected: `keep me`

19. `sbx exec sbxm-e2e-demo-claude printenv E2E_GREETING`
    Expected: `changed`

## Codex, Gemini and Pi

20. `cargo run -- new e2e-demo --profile e2e --harness codex`
    Expected: ends with `Created sandbox sbxm-e2e-demo-codex`.

21. `sbx exec sbxm-e2e-demo-codex codex debug prompt-input hi | Select-String HAZEL-6`
    Expected: at least one line containing `HAZEL-6`.

22. `cargo run -- new e2e-demo --profile e2e --harness gemini`
    Expected: `warning: skills.store is "readonly", but gemini sandboxes don't support it: …`, then
    `Created sandbox sbxm-e2e-demo-gemini`.

23. `sbx exec sbxm-e2e-demo-gemini cat /home/agent/.gemini/GEMINI.md`
    Expected: the HAZEL-6 canary line.

24. `cargo run -- new e2e-demo --profile e2e --harness pi`
    Expected: `warning: instructions.reference is set, and pi sandboxes always load it …`, then
    `Created sandbox sbxm-e2e-demo-pi` (no credential question: you approved it before).

25. `sbx exec sbxm-e2e-demo-pi cat /home/agent/.pi/agent/AGENTS.md`
    Expected: the HAZEL-6 canary line.

26. `cargo run -- list`
    Expected: four `e2e-demo` rows (claude, codex, gemini, pi), all `running` and `current`.

27. `cargo run -- doctor`
    Expected: every line `ok`, including one `project e2e-demo (<harness>, profile 'e2e')` line per harness.

## Stop and remove

28. `cargo run -- stop e2e-demo --harness codex`
    Expected: no error. `cargo run -- list` then shows the codex row as `stopped`.

29. `cargo run -- rm e2e-demo --harness gemini`
    Expected: no error. `cargo run -- list` no longer shows the gemini row; claude, codex and pi remain.

30. `cargo run -- rm e2e-demo --purge --harness pi`
    Expected: an error that `--purge` can't be used with `--harness`. Nothing is removed.

31. `cargo run -- rm e2e-demo --purge`
    Expected: a prompt `Permanently delete these directories and sandboxes?` listing `E:\sbxm-projects\e2e-demo`,
    `E:\sbxm-projects\.sbxm\e2e-demo` and the claude, codex and pi sandboxes. Answer `y`.

32. `cargo run -- list`
    Expected: no `e2e-demo` rows. `Test-Path E:\sbxm-projects\e2e-demo` prints `False`.

## Cleanup

33. `Remove-Item -Recurse $HOME\.config\sbxm\profiles\e2e`
    Expected: no output.

34. `cargo run -- doctor`
    Expected: every line `ok`, and no `e2e` lines.
