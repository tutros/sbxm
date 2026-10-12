//! `sbxm status [--watch]` (decision 171, issue 90): the Tasks section. Reads only, never calls
//! `sbx`; `backend` is threaded through unused so the Sandboxes and Comparison-runs sections
//! (separate issues, decision 171(b), (e)) can add calls here without reshaping this signature.

use std::io::{IsTerminal, Write};
use std::path::Path;
use std::time::Duration;

use anyhow::Result;

use crate::backend::SandboxBackend;
use crate::status::tasks;
use crate::task::record::ProcessProbe;

/// What `sbxm status` needs from the environment, so tests don't depend on real time, real
/// sleeps or a real terminal.
pub struct Env {
    pub now: Box<dyn Fn() -> u64>,
    pub sleep: Box<dyn Fn(Duration)>,
    /// Whether the output is a terminal: decides both whether the last column is cut to `width`
    /// (decided point 1) and whether `--watch` has any effect at all (decided point 7 of the
    /// "Smaller points" list: a non-terminal always prints exactly once).
    pub terminal: bool,
    /// Only meaningful when `terminal`; a terminal whose width couldn't be read falls back to 80.
    pub width: usize,
    /// Test-only: stop the watch loop after this many frames instead of running until the
    /// process is killed. `None` in [`Env::real`].
    pub iterations: Option<usize>,
}

impl Env {
    pub fn real() -> Self {
        Self {
            now: Box::new(crate::run::results::now),
            sleep: Box::new(std::thread::sleep),
            terminal: std::io::stdout().is_terminal(),
            width: terminal_width().unwrap_or(80),
            iterations: None,
        }
    }
}

/// The clear-and-home ANSI sequence `--watch` redraws with (decided point 7): plain ANSI, no new
/// dependency (decision 171(a)).
const CLEAR_SCREEN: &str = "\x1B[2J\x1B[H";

/// Runs the dashboard: one frame, or (with `watch`) a frame every `watch` on a terminal, cleared
/// and redrawn, until Ctrl+C (or, in a test, `env.iterations` frames). A non-terminal always
/// prints exactly one frame and returns, whether or not `watch` is set.
pub fn run(
    base: &Path,
    _backend: &dyn SandboxBackend,
    probe: &dyn ProcessProbe,
    env: &Env,
    watch: Option<Duration>,
    out: &mut dyn Write,
) -> Result<()> {
    let mut drawn = 0usize;
    loop {
        let rows = tasks::rows(base, probe)?;
        let now = (env.now)();
        let width = env.terminal.then_some(env.width);
        if env.terminal && watch.is_some() {
            write!(out, "{CLEAR_SCREEN}")?;
        }
        write!(out, "{}", tasks::render(&rows, now, width))?;
        out.flush()?;
        drawn += 1;
        let Some(interval) = watch else { break };
        if !env.terminal {
            break;
        }
        if env.iterations.is_some_and(|n| drawn >= n) {
            break;
        }
        (env.sleep)(interval);
    }
    Ok(())
}

/// The terminal's column count, queried straight from the OS with no new dependency (decision
/// 171(a)): `TIOCGWINSZ` on Unix, `GetConsoleScreenBufferInfo` on Windows (unverified here, this
/// build's primary platform is Windows but this sandbox is Linux; same pattern as
/// `task::record::path_hash_bytes`'s platform split). `None` anywhere else, or if the call fails.
#[cfg(unix)]
fn terminal_width() -> Option<usize> {
    #[repr(C)]
    struct Winsize {
        ws_row: u16,
        ws_col: u16,
        ws_xpixel: u16,
        ws_ypixel: u16,
    }
    const TIOCGWINSZ: u64 = 0x5413;
    unsafe extern "C" {
        fn ioctl(fd: i32, request: u64, ...) -> i32;
    }
    let mut size = Winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let ok = unsafe { ioctl(1, TIOCGWINSZ, std::ptr::addr_of_mut!(size)) == 0 };
    (ok && size.ws_col > 0).then_some(size.ws_col as usize)
}

#[cfg(windows)]
fn terminal_width() -> Option<usize> {
    #[repr(C)]
    struct Coord {
        x: i16,
        y: i16,
    }
    #[repr(C)]
    struct SmallRect {
        left: i16,
        top: i16,
        right: i16,
        bottom: i16,
    }
    #[repr(C)]
    struct ConsoleScreenBufferInfo {
        size: Coord,
        cursor_position: Coord,
        attributes: u16,
        window: SmallRect,
        maximum_window_size: Coord,
    }
    const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(handle: u32) -> isize;
        fn GetConsoleScreenBufferInfo(handle: isize, info: *mut ConsoleScreenBufferInfo) -> i32;
    }
    unsafe {
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        if handle == -1 {
            return None;
        }
        let mut info: ConsoleScreenBufferInfo = std::mem::zeroed();
        if GetConsoleScreenBufferInfo(handle, &mut info) == 0 {
            return None;
        }
        let width = info.window.right - info.window.left + 1;
        (width > 0).then_some(width as usize)
    }
}

#[cfg(not(any(unix, windows)))]
fn terminal_width() -> Option<usize> {
    None
}
