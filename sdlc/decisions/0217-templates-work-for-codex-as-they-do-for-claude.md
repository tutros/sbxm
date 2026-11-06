217. **Templates work for `codex` as they do for `claude` (2026-10-16, milestone 3 slice 0, a real-`sbx` check
on `sbx` 0.46.0 with `sbxm-ae857fa`; findings only, no design change):** a profile with one root install step
(`apt-get install -y jq`), a canary in its mandatory instructions and `secrets.services = ["openai"]` was created
with `sbxm new --harness codex` (13.7 s, the install step 7.0 s), saved with `sbx template save` once stopped (12.7
s, `flavor` `codex-docker`, 1.09 GB reported, about 0.2 GiB new on `C:`), and recreated with `sbx create -t TAG
--pull never --cpus 4 -m 8g --skills readonly --kit <common> --kit <harness-codex> codex <workspace>`. (a) With the
same kits: 8.4 s; marker files in `~` and `/tmp` came back, so the template's image was used; the kit's environment
won (`SBXM_CONFIG_HASH`); the install step ran again (3.1 s). (b) With a narrow `common` kit (no `setup.install`,
`allow: []`): 5.0 s; `jq` present; `codex exec` answered the canary from `~/.codex/AGENTS.md` in 6.4 s, so the
provider secret works from a template. (c) **The `codex` kit brings its own network rules:** `sbx policy ls
<sandbox> --wide` lists a kit rule allowing `github.com`, `api.github.com`, `codeload.github.com`,
`release-assets.githubusercontent.com` and `archive`/`security`/`ports.ubuntu.com`, whatever sbxm's kit says;
`example.com` and `crates.io` answered 403. So a `codex` sandbox's runtime allow list [202] can't be narrower than
those hosts (the `claude` kit's own startup `apt-get` was blocked in S11, so this is per harness). (d) Secrets,
presence only: no OpenAI key pattern in the environment, process command lines or filesystem (outside `/proc`,
`/sys`, `/dev`, `/run` and the workspace); `~/.codex/auth.json` holds `OPENAI_API_KEY` of length 13, a placeholder
the proxy swaps. (e) **An agent's session is captured by the template, observed:** the template sandbox held
`~/.codex/sessions/.../rollout-*.jsonl` from the `codex exec` run before the save, plus codex's state databases.
This confirms for `codex` the risk [210] guards against (no agent ever runs in a build sandbox); S11 had it inferred
for `claude`.
