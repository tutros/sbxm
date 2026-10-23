//! `sbxm status [--watch]` (decision 171, issue 90): the Tasks section. Reads only, never calls
//! `sbx`; `backend` is threaded through unused so the Sandboxes and Comparison-runs sections
//! (separate issues, decision 171(b), (e)) can add calls here without reshaping this signature.

#[cfg(any(unix, test))]
use std::ffi::c_ulong;
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
/// `task::record::path_hash_bytes`'s platform split). `None` anywhere else, on a Unix target
/// [`tiocgwinsz`] doesn't know (no `ioctl` call at all), or if the call fails.
#[cfg(unix)]
fn terminal_width() -> Option<usize> {
    #[repr(C)]
    struct Winsize {
        ws_row: u16,
        ws_col: u16,
        ws_xpixel: u16,
        ws_ypixel: u16,
    }
    unsafe extern "C" {
        fn ioctl(fd: std::ffi::c_int, request: c_ulong, ...) -> std::ffi::c_int;
    }
    let request = tiocgwinsz(std::env::consts::OS, std::env::consts::ARCH)?;
    let mut size = Winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let ok = unsafe { ioctl(1, request, std::ptr::addr_of_mut!(size)) == 0 };
    columns(ok, size.ws_col)
}

/// `TIOCGWINSZ`'s request value for a Unix target (issue 221), as `std::env::consts::OS` and
/// `ARCH` name it, from the `libc` crate's definitions: Linux uses `0x5413` except on mips,
/// powerpc and sparc, which share the BSD encoding `_IOR('t', 104, struct winsize)`, as macOS and
/// FreeBSD do. `None` for any other target, which then falls back to 80 columns.
#[cfg(any(unix, test))]
fn tiocgwinsz(os: &str, arch: &str) -> Option<c_ulong> {
    const LINUX_GENERIC: c_ulong = 0x5413;
    const BSD_ENCODED: c_ulong = 0x4008_7468;
    match (os, arch) {
        (
            "linux",
            "mips" | "mips64" | "mips32r6" | "mips64r6" | "powerpc" | "powerpc64" | "sparc"
            | "sparc64",
        ) => Some(BSD_ENCODED),
        ("linux", _) => Some(LINUX_GENERIC),
        ("macos" | "freebsd", _) => Some(BSD_ENCODED),
        _ => None,
    }
}

/// The width a successful probe reports; a failed call or a zero width is no answer.
#[cfg(any(unix, test))]
fn columns(ok: bool, ws_col: u16) -> Option<usize> {
    (ok && ws_col > 0).then_some(ws_col as usize)
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

#[cfg(test)]
mod tests {
    use super::{columns, tiocgwinsz};

    #[test]
    fn linux_generic_architectures_use_0x5413() {
        for arch in [
            "x86",
            "x86_64",
            "arm",
            "aarch64",
            "riscv64",
            "s390x",
            "loongarch64",
        ] {
            assert_eq!(tiocgwinsz("linux", arch), Some(0x5413), "{arch}");
        }
    }

    #[test]
    fn linux_mips_powerpc_and_sparc_use_the_bsd_encoding() {
        for arch in [
            "mips",
            "mips64",
            "mips32r6",
            "mips64r6",
            "powerpc",
            "powerpc64",
            "sparc",
            "sparc64",
        ] {
            assert_eq!(tiocgwinsz("linux", arch), Some(0x4008_7468), "{arch}");
        }
    }

    #[test]
    fn macos_and_freebsd_use_the_bsd_encoding() {
        for arch in ["x86_64", "aarch64"] {
            assert_eq!(tiocgwinsz("macos", arch), Some(0x4008_7468), "macos {arch}");
            assert_eq!(
                tiocgwinsz("freebsd", arch),
                Some(0x4008_7468),
                "freebsd {arch}"
            );
        }
    }

    #[test]
    fn other_unix_targets_get_no_request() {
        for os in [
            "netbsd", "openbsd", "solaris", "illumos", "android", "aix", "haiku",
        ] {
            assert_eq!(tiocgwinsz(os, "x86_64"), None, "{os}");
        }
    }

    #[test]
    fn a_failed_probe_or_a_zero_width_is_no_answer() {
        assert_eq!(columns(true, 132), Some(132));
        assert_eq!(columns(true, 1), Some(1));
        assert_eq!(columns(true, 0), None);
        assert_eq!(columns(false, 132), None);
    }
}
