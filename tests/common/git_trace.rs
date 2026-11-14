//! Issue 468: git's trace for the tests that rebuild a template, shown only when one fails.
//!
//! [`traced`] runs one test of the current test binary again in a child process whose
//! `GIT_TRACE2_EVENT` points at a file in a temp dir of its own (sbxm's hardened git environment
//! passes that variable through when it is set). The child gets it from its start, so no test sets
//! a variable of its own process while other tests run git. When the child fails, the panic message
//! holds its output and the trace's last [`TRACE_LINES`] lines; when it passes, nothing is printed.

use std::path::Path;
use std::process::Command;

/// How many of the trace's last lines a failure shows.
pub const TRACE_LINES: usize = 400;

/// Set in the child process [`run_traced`] starts.
const CHILD: &str = "SBXM_TEST_TRACED_RUN";

/// Whether this process is the child of a [`run_traced`]: the test body runs here.
pub fn in_traced_run() -> bool {
    std::env::var_os(CHILD).is_some()
}

/// In a traced run, runs `body`; otherwise runs test `name` (the calling test's own name, with its
/// module: `task_template_behind_main::...` in the one test binary, issue 473) as a traced run
/// and panics with its failure.
pub fn traced(name: &str, body: impl FnOnce()) {
    if in_traced_run() {
        body();
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    if let Err(failure) = run_traced_in(dir.path(), name) {
        // Kept, so the trace file the failure names is still there to read in full.
        let _ = dir.keep();
        panic!("{failure}");
    }
}

/// Runs test `name` of the current test binary in a child process with git's trace in a file of
/// its own. `Err` is the failure to show: the child's output, then the trace's last lines.
/// The trace file goes in a temp dir removed afterwards.
pub fn run_traced(name: &str) -> Result<(), String> {
    run_traced_in(tempfile::tempdir().unwrap().path(), name)
}

fn run_traced_in(dir: &Path, name: &str) -> Result<(), String> {
    let trace = dir.join("git-trace2.json");
    let out = Command::new(std::env::current_exe().unwrap())
        .args([name, "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env("GIT_TRACE2_EVENT", &trace)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    // `--exact` with a name no test has runs nothing, and passes.
    if out.status.success() && stdout.contains("1 passed") {
        return Ok(());
    }
    Err(format!(
        "test {name} failed in its traced run ({}):\n{stdout}\n{}\n\
         --- the last lines of git's trace ({}, at most {TRACE_LINES}) ---\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr),
        trace.display(),
        trace_tail(&trace)
    ))
}

/// The last [`TRACE_LINES`] lines of the trace at `path`, or "no trace written" when it is
/// missing or holds nothing.
pub fn trace_tail(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes);
    if text.trim().is_empty() {
        return "no trace written".to_owned();
    }
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(TRACE_LINES)..].join("\n")
}
