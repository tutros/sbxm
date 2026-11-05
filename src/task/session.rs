//! The Claude worker's own session, kept for its fix rounds (issue 418, decision 285): with
//! `[fix] resume = true`, the session a Claude worker's run used is copied out of its sandbox
//! into the task's folder as `worker-session.jsonl` once the run ends (so it outlives the
//! sandbox, released or removed), and a fix round puts it back under `~/.claude/projects/` and
//! runs `claude -p --resume <session>`. Anything that stops that makes the round run fresh, as
//! before, with a line saying why; it never fails the task.

use std::fs;
use std::path::Path;

use anyhow::Result;

use super::pipeline::{AGENT_DIR, Prepared, TaskEnv, copy_out_staged};
use super::record;
use super::repo::{self, AgentFile, Existing};
use crate::backend::{ExecSpec, Location, SandboxBackend, Stdin};
use crate::harness::Harness;
use crate::headless::{HeadlessResult, RunStatus};

/// The kept session's name, in the task's folder and, on its way in and out, in the clone's
/// `.sbxm-task/`.
pub const FILE: &str = "worker-session.jsonl";

/// The biggest session kept (decided point 2): one byte over, and the fix round runs fresh.
pub const CAP: u64 = 64 * 1024 * 1024;

/// What the `claude` CLI says, exiting 1 before any model call, for a session it has no file for
/// (checked by hand on 2026-11-04).
const NOT_FOUND: &str = "No conversation found with session ID";

/// Shell functions the session scripts share, so nothing they write or remove goes through a link
/// the agent left (the clone and `~/.claude` are the agent's; review M-1): `plain_dir` refuses a
/// folder that is a link or no folder, `free_file` a file that is a link or no plain file, and
/// `put` copies into a fresh file (`mktemp`) and renames it over the destination, so an existing
/// plain file, hard links included, is replaced rather than written through.
const GUARDS: &str = concat!(
    "set -e; ",
    "plain_dir() { if [ -L \"$1\" ] || [ ! -d \"$1\" ]; then ",
    "echo \"$1 is a link or not a folder; refusing to write into it\" >&2; exit 1; fi; }; ",
    "free_file() { if [ -L \"$1\" ] || { [ -e \"$1\" ] && [ ! -f \"$1\" ]; }; then ",
    "echo \"$1 is a link or not a file; refusing to write it\" >&2; exit 1; fi; }; ",
    "put() { t=$(mktemp \"${2%/*}/.sbxm-session.XXXXXX\"); chmod 644 \"$t\"; ",
    "cp -- \"$1\" \"$t\" && mv -f -- \"$t\" \"$2\" || { rm -f -- \"$t\"; exit 1; }; }; ",
);

/// Copies the session out of the CLI's projects folder into the clone, run by `sh -c` with `$1`
/// that folder (`$HOME/.claude/projects`), `$2` the session id and `$3` the copy's path. The
/// session is found by its file name, whatever project folder the CLI chose for it.
const SAVE_SCRIPT: &str = concat!(
    "f=$(find \"$1\" -mindepth 2 -maxdepth 2 -type f -name \"$2.jsonl\" 2>/dev/null | head -n 1); ",
    "if [ -z \"$f\" ]; then echo \"no $2.jsonl under $1\" >&2; exit 1; fi; ",
    "plain_dir \"${3%/*}\"; free_file \"$3\"; put \"$f\" \"$3\"",
);

/// Removes `$1`, a file in the clone's `.sbxm-task/`, unless that folder is a link.
const REMOVE_SCRIPT: &str = "plain_dir \"${1%/*}\"; rm -f -- \"$1\"";

/// Puts the session back where the CLI looks for it, run by `sh -c` with `$1` the clone's project
/// folder under `$HOME/.claude/projects`, `$2` the session id and `$3` the copy in the clone
/// (removed once in place).
const RESTORE_SCRIPT: &str = concat!(
    "[ -e \"$1\" ] || [ -L \"$1\" ] || mkdir -p -- \"$1\"; ",
    "plain_dir \"$1\"; free_file \"$1/$2.jsonl\"; ",
    "plain_dir \"${3%/*}\"; if [ -L \"$3\" ] || [ ! -f \"$3\" ]; then ",
    "echo \"$3 is a link or not a file; refusing to read it\" >&2; exit 1; fi; ",
    "put \"$3\" \"$1/$2.jsonl\"; ",
    "rm -f -- \"$3\"",
);

/// Clears the way for a copy into a cloud clone's `.sbxm-task/` at `$1`: refused when that folder
/// is a link or `$1` is a link or no plain file, removed when it is one, so the copy makes a new
/// file instead of writing through what the agent left there.
const CLEAR_SCRIPT: &str = "plain_dir \"${1%/*}\"; free_file \"$1\"; rm -f -- \"$1\"";

/// The CLI's projects folder in `sandbox`, `$HOME/.claude/projects`, with `$HOME` read once
/// (`printenv HOME`) and passed to the scripts as a value.
fn projects(backend: &dyn SandboxBackend, sandbox: &str) -> std::result::Result<String, String> {
    let out = backend
        .exec(
            sandbox,
            &ExecSpec {
                workdir: None,
                argv: vec!["printenv".to_owned(), "HOME".to_owned()],
                stdin: Stdin::Closed,
            },
        )
        .map_err(|e| format!("cannot read $HOME in {sandbox}: {e:#}"))?;
    let home = out.stdout.trim_end_matches(['\n', '\r']);
    if out.exit_code != Some(0)
        || !home.starts_with('/')
        || home.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(format!(
            "{sandbox} reports HOME as {home:?}, which isn't an absolute path"
        ));
    }
    Ok(format!("{}/.claude/projects", home.trim_end_matches('/')))
}

/// Whether the task's fix rounds resume the worker's session: the switch is on and the worker is
/// Claude. Every other harness runs its fix rounds fresh, as before.
fn applies(env: &TaskEnv) -> bool {
    env.config.fix_resume && env.config.worker.harness == Harness::Claude
}

/// A session id sbxm passes on: Claude's are UUIDs, so letters, digits and `-` only, which also
/// keeps it safe as a file name and a `find -name` pattern.
fn is_session_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// The folder under `~/.claude/projects/` the CLI keeps a working folder's sessions in: the path
/// with every character but an ASCII letter or digit turned into `-`.
fn project_dir(clone: &str) -> String {
    clone
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// After a run of the worker in `sandbox` (its first run or a fix round), while the sandbox is
/// still there: keeps the session it used, `id`, as `worker-session.jsonl` in the task's folder
/// and records its id. A session that can't be kept (none named, none found, empty, over
/// [`CAP`]) leaves no file and no id, and a line saying a fix round will run fresh. Nothing
/// happens unless [`applies`]. `Err` only when the record can't be written.
pub(super) fn keep(
    env: &TaskEnv,
    prepared: &mut Prepared,
    sandbox: &str,
    id: Option<&str>,
) -> Result<()> {
    if !applies(env) {
        return Ok(());
    }
    let kept_at = prepared.meta.join(FILE);
    let kept = copy_out(env.backend, prepared, sandbox, id).and_then(|text| {
        fs::write(&kept_at, text).map_err(|e| format!("cannot write {}: {e}", kept_at.display()))
    });
    let session = match kept {
        Ok(()) => id.map(str::to_owned),
        Err(why) => {
            let _ = fs::remove_file(&kept_at);
            prepared.events.push(format!(
                "{}: the worker's session was not kept ({why}); a fix round will run fresh",
                prepared.record.id
            ));
            None
        }
    };
    if let Some(worker) = prepared.record.worker.as_mut() {
        worker.session = session;
    }
    record::write(&prepared.meta, &prepared.record)
}

/// The session's text, copied out of the sandbox through the clone's `.sbxm-task/` and read with
/// the link-free, size-capped reader; `Err` says why there is none.
fn copy_out(
    backend: &dyn SandboxBackend,
    prepared: &Prepared,
    sandbox: &str,
    id: Option<&str>,
) -> std::result::Result<String, String> {
    let id = id.ok_or("the run named no session")?;
    if !is_session_id(id) {
        return Err(format!(
            "the run's session id {id:?} isn't one sbxm passes on"
        ));
    }
    let clone = prepared.sandbox_clone().to_string_lossy().into_owned();
    let in_clone = format!("{clone}/{AGENT_DIR}/{FILE}");
    let projects = projects(backend, sandbox)?;
    let out = backend
        .exec(sandbox, &script(SAVE_SCRIPT, &[&projects, id, &in_clone]))
        .map_err(|e| format!("cannot copy it out of {sandbox}: {e:#}"))?;
    if out.exit_code != Some(0) {
        return Err(format!(
            "it could not be copied out of {sandbox}: {}",
            out.stderr.trim()
        ));
    }
    let read = match prepared.record.location {
        Location::Local => repo::read_agent_file(&prepared.workspace, FILE, CAP),
        Location::Cloud => copy_out_staged(backend, &prepared.meta, sandbox, &clone, FILE)
            .and_then(|staged| repo::read_agent_file(staged.dir(), FILE, CAP)),
    };
    // The copy in the clone goes (the task's folder keeps its own), removed inside the sandbox:
    // the clone is the agent's, so the host never deletes through what it may have linked there.
    let _ = backend.exec(sandbox, &script(REMOVE_SCRIPT, &[&in_clone]));
    match read {
        Ok(AgentFile::Text(text)) if text.is_empty() => Err("it is empty".to_owned()),
        Ok(AgentFile::Text(text)) => Ok(text),
        Ok(AgentFile::Missing) => Err("no copy came out of the sandbox".to_owned()),
        Ok(AgentFile::TooLarge(len)) => Err(format!("it is {len} bytes, over the 64 MiB limit")),
        Err(e) => Err(format!("its copy was refused: {e:#}")),
    }
}

/// Before a fix round in `sandbox`: puts the kept session back where the CLI finds it and returns
/// its id for `--resume`. `None` (with a line saying the round runs fresh and why) when there is
/// none to resume or it can't be put back; `None` without a word unless [`applies`].
pub(super) fn restore(env: &TaskEnv, prepared: &mut Prepared, sandbox: &str) -> Option<String> {
    if !applies(env) {
        return None;
    }
    match put_back(env.backend, prepared, sandbox) {
        Ok(id) => Some(id),
        Err(why) => {
            prepared.events.push(format!(
                "{}: the fix round runs fresh: {why}",
                prepared.record.id
            ));
            None
        }
    }
}

fn put_back(
    backend: &dyn SandboxBackend,
    prepared: &Prepared,
    sandbox: &str,
) -> std::result::Result<String, String> {
    let id = prepared
        .record
        .worker
        .as_ref()
        .and_then(|w| w.session.clone())
        .ok_or("no session of the worker was kept")?;
    if !is_session_id(&id) {
        return Err(format!(
            "the kept session id {id:?} isn't one sbxm passes on"
        ));
    }
    let kept_at = prepared.meta.join(FILE);
    let len = match fs::symlink_metadata(&kept_at) {
        Ok(meta) if meta.file_type().is_file() => meta.len(),
        _ => return Err(format!("the kept session {} is missing", kept_at.display())),
    };
    if len == 0 {
        return Err(format!("the kept session {} is empty", kept_at.display()));
    }
    if len > CAP {
        return Err(format!(
            "the kept session {} is {len} bytes, over the 64 MiB limit",
            kept_at.display()
        ));
    }
    let clone = prepared.sandbox_clone().to_string_lossy().into_owned();
    let in_clone = format!("{clone}/{AGENT_DIR}/{FILE}");
    let projects = projects(backend, sandbox)?;
    place(backend, prepared, sandbox, &kept_at, &in_clone)?;
    let out = backend
        .exec(
            sandbox,
            &script(
                RESTORE_SCRIPT,
                &[
                    &format!("{projects}/{}", project_dir(&clone)),
                    &id,
                    &in_clone,
                ],
            ),
        )
        .map_err(|e| format!("cannot put the session back in {sandbox}: {e:#}"))?;
    if out.exit_code != Some(0) {
        return Err(format!(
            "the session could not be put back in {sandbox}: {}",
            out.stderr.trim()
        ));
    }
    Ok(id)
}

/// The kept session into the clone's `.sbxm-task/`: written into a local clone (refusing a link
/// the agent left), copied into a cloud one.
fn place(
    backend: &dyn SandboxBackend,
    prepared: &Prepared,
    sandbox: &str,
    kept_at: &Path,
    in_clone: &str,
) -> std::result::Result<(), String> {
    match prepared.record.location {
        Location::Local => {
            let bytes =
                fs::read(kept_at).map_err(|e| format!("cannot read {}: {e}", kept_at.display()))?;
            repo::write_agent_files(&prepared.workspace, &[(FILE, &bytes)], Existing::Refuse)
                .map_err(|e| format!("{e:#}"))
        }
        Location::Cloud => {
            let out = backend
                .exec(sandbox, &script(CLEAR_SCRIPT, &[in_clone]))
                .map_err(|e| format!("cannot copy the session into {sandbox}: {e:#}"))?;
            if out.exit_code != Some(0) {
                return Err(format!(
                    "the session could not be copied into {sandbox}: {}",
                    out.stderr.trim()
                ));
            }
            backend
                .copy_in(sandbox, kept_at, in_clone)
                .map_err(|e| format!("cannot copy the session into {sandbox}: {e:#}"))
        }
    }
}

/// Whether a resumed run failed because the CLI had no such session: the round runs again fresh.
pub(super) fn not_found(result: &HeadlessResult) -> bool {
    let RunStatus::Failed(why) = &result.status else {
        return false;
    };
    // On stderr it is in the failure; on stdout it is a plain line, not inside a JSON event.
    why.contains(NOT_FOUND)
        || result
            .transcript
            .lines()
            .any(|line| line.trim_start().starts_with(NOT_FOUND))
}

/// `sh -c <GUARDS><script> sh <args…>`, with no working folder: the values are positional
/// arguments, never pasted into the script.
fn script(script: &str, args: &[&str]) -> ExecSpec {
    let mut argv: Vec<String> = ["sh", "-c", &format!("{GUARDS}{script}"), "sh"]
        .map(str::to_owned)
        .into();
    argv.extend(args.iter().map(|a| (*a).to_owned()));
    ExecSpec {
        workdir: None,
        argv,
        stdin: Stdin::Closed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_working_folder_becomes_the_clis_project_folder_name() {
        assert_eq!(
            project_dir("/h/sbxm-e2e/resume-check"),
            "-h-sbxm-e2e-resume-check"
        );
        assert_eq!(
            project_dir("/home/agent/work/issue_4.x"),
            "-home-agent-work-issue-4-x"
        );
    }

    fn failed(why: &str, transcript: &str) -> HeadlessResult {
        HeadlessResult {
            status: RunStatus::Failed(why.to_owned()),
            answer: String::new(),
            transcript: transcript.to_owned(),
            usage: crate::headless::Usage {
                input_tokens: 0,
                output_tokens: 0,
                cost_usd: None,
            },
            session_id: None,
        }
    }

    #[test]
    fn only_the_clis_own_message_on_a_failed_run_is_a_session_not_found() {
        let message = "No conversation found with session ID: 92ae7cb6";
        // On stderr (the failure says it) or as a plain line on stdout.
        assert!(not_found(&failed(&format!("exit code 1: {message}"), "")));
        assert!(not_found(&failed("exit code 1", &format!("{message}\n"))));
        // A run that completed, or failed for another reason, whose conversation only mentions it.
        let quoted = format!("{{\"type\":\"assistant\",\"text\":\"{message}\"}}\n");
        let mut completed = failed("", &quoted);
        completed.status = RunStatus::Completed;
        assert!(!not_found(&completed));
        assert!(!not_found(&failed("exit code 1", &quoted)));
    }

    #[test]
    fn only_uuid_like_ids_are_passed_on() {
        assert!(is_session_id("92ae7cb6-403a-4abc-a295-072f335d20cb"));
        for bad in ["", "a b", "../x", "a*", "x;rm", &"a".repeat(129)] {
            assert!(!is_session_id(bad), "{bad:?}");
        }
    }
}
