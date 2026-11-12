//! Recovery hints that say where and with what to run them (issue 574): a hint names a command
//! (`sbxm task resume --issue 468`), and the user would otherwise have to ask which directory,
//! which executable and which config dir to run it with.
//!
//! One place does it, at output time, instead of every hint string: [`rewrite`] turns the bare
//! `sbxm` of a command into the running executable's full path, quoted for the shell of this
//! platform (PowerShell on Windows, POSIX sh elsewhere), and [`block`] is the short block printed
//! once after the command's output, with the `cd` line and, when `SBXM_CONFIG_DIR` is set, the
//! line that sets it. [`Local`] is the writer that applies the rewrite to a command's screen
//! output (and so its `run.log`) and prints the block right after the first hint; [`named`] is
//! the rewrite for the few hints of a command whose output also shows stored content (a run's
//! answers, a task's log, a title), which is never rewritten.
//!
//! Only local output goes through here: a PR comment or anything else posted to GitHub keeps the
//! bare `sbxm` (these paths are this machine's), and so do the loop's toasts.

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// The top-level subcommands of `sbxm` (`crate::cli::Command`; a test keeps the two in step):
/// `sbxm ` followed by one of them is a command, anything else (`sbxm denied ...`) is prose.
const SUBCOMMANDS: &[&str] = &[
    "new", "list", "open", "stop", "rm", "doctor", "run", "task", "config", "template", "status",
];

/// The subcommand names [`rewrite`] recognises, for the test that compares them with the CLI.
pub fn subcommands() -> &'static [&'static str] {
    SUBCOMMANDS
}

/// The shell a printed line is written for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    PowerShell,
    Sh,
}

impl Shell {
    /// This platform's shell, chosen at compile time.
    #[cfg(windows)]
    pub const HOST: Shell = Shell::PowerShell;
    #[cfg(not(windows))]
    pub const HOST: Shell = Shell::Sh;

    /// `text` as one single-quoted word, with this shell's own escaping of a single quote:
    /// doubled in PowerShell (which also takes the typographic single quotes as quotes), closed,
    /// escaped and reopened in sh.
    pub fn quote(self, text: &str) -> String {
        let mut quoted = String::with_capacity(text.len() + 2);
        quoted.push('\'');
        for c in text.chars() {
            match (self, c) {
                (Shell::PowerShell, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') => {
                    quoted.push(c);
                    quoted.push(c);
                }
                (Shell::Sh, '\'') => quoted.push_str("'\\''"),
                _ => quoted.push(c),
            }
        }
        quoted.push('\'');
        quoted
    }

    /// What a command line starts with in place of `sbxm`: the executable, quoted (behind
    /// PowerShell's call operator, which a quoted path needs to run).
    pub fn program(self, exe: &str) -> String {
        match self {
            Shell::PowerShell => format!("& {}", self.quote(exe)),
            Shell::Sh => self.quote(exe),
        }
    }

    /// The line that changes to `dir`. PowerShell's `cd` takes a path as a wildcard pattern even
    /// single-quoted (a folder named `[repo]` matches nothing), so it gets `-LiteralPath`.
    pub fn cd(self, dir: &str) -> String {
        match self {
            Shell::PowerShell => format!("cd -LiteralPath {}", self.quote(dir)),
            Shell::Sh => format!("cd {}", self.quote(dir)),
        }
    }

    /// The line that sets `SBXM_CONFIG_DIR` to `value`.
    pub fn set_config_dir(self, value: &str) -> String {
        match self {
            Shell::PowerShell => format!("$env:SBXM_CONFIG_DIR={}", self.quote(value)),
            Shell::Sh => format!("export SBXM_CONFIG_DIR={}", self.quote(value)),
        }
    }
}

/// What a hint's commands need to run as the failed command ran.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Context {
    /// The directory to run them in: the working directory, which is the repo root whose
    /// `sbxm-task.toml` a task command read. `None` when it can't be read.
    pub dir: Option<PathBuf>,
    /// The working directory the command ran in, which a relative `config_dir` is relative to;
    /// `None` when it can't be read.
    pub cwd: Option<PathBuf>,
    /// The running executable, `None` when `std::env::current_exe` fails (the hint then keeps
    /// the bare `sbxm`).
    pub exe: Option<PathBuf>,
    /// `SBXM_CONFIG_DIR` as given, when it is set.
    pub config_dir: Option<OsString>,
}

impl Context {
    /// This process's context: the directory [`set_dir`] named, else the working directory.
    pub fn current() -> Self {
        let named = DIR.lock().ok().and_then(|dir| dir.clone());
        let cwd = std::env::current_dir().ok();
        Self {
            dir: named.or_else(|| cwd.clone()),
            cwd,
            exe: std::env::current_exe().ok(),
            config_dir: std::env::var_os("SBXM_CONFIG_DIR"),
        }
    }
}

/// `text` with the bare `sbxm` of every command in it replaced by `program` (see
/// [`Shell::program`]); `None` when it holds no command. A command is `sbxm ` followed by one of
/// the CLI's subcommands, at a line's start after indentation, after `next: `, or right after a
/// backtick; `sbxm` anywhere else (prose, `sbxm-task.toml`, `.sbxm`, `sbxm-dev`) stays.
pub fn rewrite(text: &str, program: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    for line in text.split_inclusive('\n') {
        let starts = command_starts(line);
        if starts.is_empty() {
            out.push_str(line);
            continue;
        }
        changed = true;
        let mut from = 0;
        for start in starts {
            out.push_str(&line[from..start]);
            out.push_str(program);
            from = start + "sbxm".len();
        }
        out.push_str(&line[from..]);
    }
    changed.then_some(out)
}

/// The byte offsets in `line` where a command's `sbxm` starts, in order.
fn command_starts(line: &str) -> Vec<usize> {
    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    let mut candidates = vec![indent];
    candidates.extend(line.match_indices("next: ").map(|(i, m)| i + m.len()));
    candidates.extend(line.match_indices('`').map(|(i, _)| i + 1));
    candidates.sort_unstable();
    candidates.dedup();
    candidates
        .into_iter()
        .filter(|&i| is_command(&line[i..]))
        .collect()
}

/// Whether `rest` starts with `sbxm <subcommand>`, the subcommand ending at a word boundary.
fn is_command(rest: &str) -> bool {
    let Some(after) = rest.strip_prefix("sbxm ") else {
        return false;
    };
    let end = after
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .unwrap_or(after.len());
    SUBCOMMANDS.contains(&&after[..end])
}

/// The block printed once after output whose commands were rewritten: the `cd` line, and the
/// line that sets `SBXM_CONFIG_DIR` when it is set; empty when there is neither.
pub fn block(context: &Context, shell: Shell) -> String {
    let mut lines = Vec::new();
    if let Some(dir) = &context.dir {
        lines.push(shell.cd(&dir.to_string_lossy()));
    }
    if let Some(value) = &context.config_dir {
        lines.push(shell.set_config_dir(&config_dir_value(context, value)));
    }
    if lines.is_empty() {
        return String::new();
    }
    let mut text = "to run the commands above as this one ran, first:\n".to_owned();
    for line in lines {
        text.push_str("  ");
        text.push_str(&line);
        text.push('\n');
    }
    text
}

/// The `SBXM_CONFIG_DIR` value the block prints (issue 589): as given, unless it is relative and
/// the block's `cd` goes to another directory than the one the command ran in; then the working
/// directory joined with it, so the printed lines select the same config after the `cd`. Only
/// `.` segments are dropped: a `..` is left for the OS, which resolves it after any link before
/// it, as it did for the command.
fn config_dir_value(context: &Context, value: &OsString) -> String {
    let given = Path::new(value);
    match (&context.dir, &context.cwd) {
        (Some(dir), Some(cwd)) if given.is_relative() && !same_dir(dir, cwd) => cwd
            .join(given)
            .components()
            .collect::<PathBuf>()
            .to_string_lossy()
            .into_owned(),
        _ => value.to_string_lossy().into_owned(),
    }
}

/// Whether `a` and `b` name one directory: equal, or equal once links are resolved.
fn same_dir(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (std::fs::canonicalize(a), std::fs::canonicalize(b)),
            (Ok(a), Ok(b)) if a == b
        )
}

/// Set when this process printed a rewritten command (or named a directory) and the block that
/// says where to run it is owed: [`take_block`] and [`take_frame_block`] pay it.
static OWED: AtomicBool = AtomicBool::new(false);

/// Set once the block was printed, so an ordinary command prints it once.
static SHOWN: AtomicBool = AtomicBool::new(false);

/// Whether [`named`] rewrites: the binary turns it on at its start; the library's own callers
/// (tests) leave it off and see the bare `sbxm`.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Turns on [`named`] for this process.
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

/// A hint generated by a command that shows stored content among its own lines (the dashboard,
/// `task status`): [`local`] when [`enable`]d, else `text` unchanged. Such a command's output as
/// a whole isn't rewritten, so a stored answer, title or log line stays as it was stored.
pub fn named(text: &str) -> String {
    if ENABLED.load(Ordering::Relaxed) {
        local(text)
    } else {
        text.to_owned()
    }
}

/// The directory the block names instead of the working directory, see [`set_dir`].
static DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Names `dir` as where this command should have run: the checkout a task was started in, when
/// it was run where there is no `sbxm-task.toml`. The block then names it, and is printed even
/// if no command was named.
pub fn set_dir(dir: &Path) {
    if let Ok(mut named) = DIR.lock() {
        *named = Some(dir.to_owned());
    }
    OWED.store(true, Ordering::Relaxed);
}

/// [`rewrite`] for the executable `exe` in `shell`'s syntax; `None` when `text` holds no command,
/// and when there is no executable to name (`current_exe` failed), so the bare `sbxm` stays.
pub fn rewrite_for(text: &str, exe: Option<&Path>, shell: Shell) -> Option<String> {
    rewrite(text, &shell.program(&exe?.to_string_lossy()))
}

/// `text` as this process prints it locally: [`rewrite_for`] the running executable and this
/// platform's shell, or `text` unchanged.
pub fn local(text: &str) -> String {
    let exe = std::env::current_exe().ok();
    match rewrite_for(text, exe.as_deref(), Shell::HOST) {
        Some(rewritten) => {
            OWED.store(true, Ordering::Relaxed);
            rewritten
        }
        None => text.to_owned(),
    }
}

/// The context block, the first time it is asked for after [`local`] rewrote a command; `None`
/// before that, once it was given, and when the block would be empty. [`Local`] asks after every
/// line it writes, so a command that goes on running shows it right after its first hint.
pub fn take_block() -> Option<String> {
    if !OWED.swap(false, Ordering::Relaxed) || SHOWN.swap(true, Ordering::Relaxed) {
        return None;
    }
    Some(block(&Context::current(), Shell::HOST)).filter(|b| !b.is_empty())
}

/// [`take_block`] for one frame of a dashboard that redraws: given for every frame with a hint,
/// since the next frame draws over the last one.
pub fn take_frame_block() -> Option<String> {
    if !OWED.swap(false, Ordering::Relaxed) {
        return None;
    }
    SHOWN.store(true, Ordering::Relaxed);
    Some(block(&Context::current(), Shell::HOST)).filter(|b| !b.is_empty())
}

/// A writer that passes its output through [`local`] a line at a time: what a command prints on
/// the screen (and, through a `Tee`, into its `run.log`). A partial line goes out on `flush`.
pub struct Local<W: Write> {
    inner: W,
    pending: Vec<u8>,
}

impl<W: Write> Local<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            pending: Vec::new(),
        }
    }

    /// Writes out the pending bytes up to `end`, rewritten when they are text, then the block if
    /// it is owed (and not printed yet).
    fn emit(&mut self, end: usize) -> io::Result<()> {
        let chunk: Vec<u8> = self.pending.drain(..end).collect();
        match std::str::from_utf8(&chunk) {
            Ok(text) => self.inner.write_all(local(text).as_bytes())?,
            Err(_) => self.inner.write_all(&chunk)?,
        }
        match take_block() {
            Some(block) => self.inner.write_all(block.as_bytes()),
            None => Ok(()),
        }
    }
}

impl<W: Write> Write for Local<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(buf);
        if let Some(last) = self.pending.iter().rposition(|&b| b == b'\n') {
            self.emit(last + 1)?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let end = self.pending.len();
        self.emit(end)?;
        self.inner.flush()
    }
}

impl<W: Write> Drop for Local<W> {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}
