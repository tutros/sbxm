//! `sbxm status [--watch]` (decision 171, issue 90): the Tasks section, then the running cloud
//! sandboxes (milestone 4 slice 8, decision 259), the local sandboxes (issue 92), the comparison
//! runs, the Loop and the disk footer. Changes nothing: the `sbx` calls are the cloud instance's `ls`
//! and the local instance's, once per frame each and in parallel, and a list that can't be read is
//! one line, not an error (P17 (a)). Each section is written as its read returns (issue 577).

#[cfg(any(unix, test))]
use std::ffi::c_ulong;
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::time::Duration;

use anyhow::Result;

use crate::backend::SandboxBackend;
use crate::config::GlobalConfig;
use crate::github::GitHubBackend;
use crate::slot;
use crate::status::{cloud, footer, looping, runs, sandboxes, tasks};
use crate::task::record::ProcessProbe;

/// What `sbxm status` needs from the environment, so tests don't depend on real time, real
/// sleeps or a real terminal.
pub struct Env {
    pub now: Box<dyn Fn() -> u64>,
    pub sleep: Box<dyn Fn(Duration)>,
    /// A monotonic reading (from any fixed origin) that the wait to the next draw is measured
    /// with: `now` counts whole epoch seconds, too coarse for a draw of a few seconds (issue 577).
    pub monotonic: Box<dyn Fn() -> Duration>,
    /// Whether the output is a terminal: decides both whether the last column is cut to `width`
    /// (decided point 1) and whether `--watch` has any effect at all (decided point 7 of the
    /// "Smaller points" list: a non-terminal always prints exactly once).
    pub terminal: bool,
    /// Only meaningful when `terminal`; a terminal whose width couldn't be read falls back to 80.
    pub width: usize,
    /// Test-only: stop the watch loop after this many frames instead of running until the
    /// process is killed. `None` in [`Env::real`].
    pub iterations: Option<usize>,
    /// What the disk footer reads the drives with (issue 92).
    pub disk: footer::Disk,
    /// `sandbox_slots` and the slots' folder, when set: one line names who holds them
    /// (decision 304).
    pub slots: Option<(u32, slot::Gate)>,
    /// `cloud_slots`, when set: a second line names who holds the cloud slots (issue 599).
    pub cloud_slots: Option<(u32, slot::Gate)>,
}

impl Env {
    pub fn real(config: &GlobalConfig) -> Self {
        Self {
            now: Box::new(crate::run::results::now),
            sleep: Box::new(std::thread::sleep),
            monotonic: {
                let origin = std::time::Instant::now();
                Box::new(move || origin.elapsed())
            },
            terminal: std::io::stdout().is_terminal(),
            width: terminal_width().unwrap_or(80),
            iterations: None,
            disk: footer::Disk::real(config),
            slots: config
                .resources
                .sandbox_slots
                .map(|count| (count, slot::Gate::default())),
            cloud_slots: config
                .resources
                .cloud_slots
                .map(|count| (count, slot::Gate::default())),
        }
    }
}

/// The clear-and-home ANSI sequence `--watch` redraws with (decided point 7): plain ANSI, no new
/// dependency (decision 171(a)).
const CLEAR_SCREEN: &str = "\x1B[2J\x1B[H";
/// Cursor home, and erase to the end of the screen: what later `--watch` frames overwrite with.
const CURSOR_HOME: &str = "\x1B[H";
const ERASE_BELOW: &str = "\x1B[J";

/// What the Loop section needs to read the queue and the PRs awaiting the user (decision 294).
pub struct LoopSource<'a> {
    pub github: &'a dyn GitHubBackend,
    /// The repo the loop works on (`owner/name`, the checkout's origin), or why it is unknown.
    pub repo: Result<String, String>,
}

/// The repo of the current directory's checkout (its origin), or the first line of why it can't
/// be told: what the Loop section's queue is read from.
pub fn checkout_repo() -> Result<String, String> {
    let first = |e: String| e.lines().next().unwrap_or_default().to_owned();
    let dir = std::env::current_dir().map_err(|e| first(e.to_string()))?;
    super::task_start::resolve_repo(None, &dir).map_err(|e| first(format!("{e:#}")))
}

/// [`run_with`] without the Loop section: the Tasks table and the cloud sandboxes.
pub fn run(
    base: &Path,
    backend: &dyn SandboxBackend,
    probe: &dyn ProcessProbe,
    env: &Env,
    watch: Option<Duration>,
    out: &mut dyn Write,
) -> Result<()> {
    run_with(base, backend, probe, env, watch, None, out)
}

/// Runs the dashboard: one frame, or (with `watch`) a frame every `watch` on a terminal (measured
/// from the start of a frame), the first clearing the screen and the later ones overwriting it in
/// place, until Ctrl+C (or, in a test, `env.iterations` frames). A non-terminal always
/// prints exactly one frame and returns, whether or not `watch` is set.
pub fn run_with(
    base: &Path,
    backend: &dyn SandboxBackend,
    probe: &dyn ProcessProbe,
    env: &Env,
    watch: Option<Duration>,
    loop_source: Option<&LoopSource>,
    out: &mut dyn Write,
) -> Result<()> {
    let mut drawn = 0usize;
    let mut cache = looping::Cache::default();
    loop {
        let started = (env.now)();
        let started_at = (env.monotonic)();
        let width = env.terminal.then_some(env.width);
        // Only a terminal's `--watch` redraws; the first frame clears the screen, later ones
        // overwrite the previous frame in place (issue 577).
        let mut frame = Frame {
            out: &mut *out,
            mode: match (env.terminal && watch.is_some(), drawn) {
                (false, _) => Mode::Plain,
                (true, 0) => Mode::Clear,
                (true, _) => Mode::InPlace,
            },
        };
        frame.start()?;
        let rows = tasks::rows(base, probe)?;
        frame.section(&tasks::render(&rows, started, width))?;
        // The two listings run side by side; the local one in a worker, so the cloud section is
        // written as soon as its read returns, whatever the local one still takes.
        let local = std::thread::scope(|scope| -> Result<_> {
            let local_read = scope.spawn(|| sandboxes::read(backend));
            let in_cloud = cloud::read(backend);
            frame.section(&cloud::render(&in_cloud, started, width))?;
            Ok(local_read.join().expect("the local listing panicked"))
        })?;
        let claims = sandboxes::claims(base);
        frame.section(&sandboxes::render(&local, &claims, started, width))?;
        // The runs read the same local list, never a second `sbx ls`.
        let comparisons = runs::read_with(base, &|| match &local {
            sandboxes::Local::Sandboxes(all) => Some(all.iter().map(|s| s.name.clone()).collect()),
            sandboxes::Local::Unreadable(_) => None,
        });
        frame.section(&runs::render(&comparisons, started, width))?;
        if let Some(source) = loop_source {
            let cloud_slots = env.cloud_slots.as_ref().map(|(count, _)| *count);
            let view = loop_view(base, probe, source, cloud_slots, &mut cache, started);
            frame.section(&looping::render(&view, width))?;
        }
        if let Some((count, gate)) = &env.slots {
            frame.section(&format!("{}\n", slot::status_line(*count, gate)))?;
        }
        if let Some((count, gate)) = &env.cloud_slots {
            frame.section(&format!("{}\n", slot::cloud_status_line(*count, gate)))?;
        }
        let drives = footer::read(base, &env.disk);
        frame.section(&footer::render(&drives, env.disk.min_free_disk, width))?;
        // Where to run the frame's hints, in every frame that has one: the next draws over it.
        if let Some(block) = crate::hint::take_frame_block() {
            frame.section(&format!("\n{block}"))?;
        }
        frame.finish()?;
        drawn += 1;
        let Some(interval) = watch else { break };
        if !env.terminal {
            break;
        }
        if env.iterations.is_some_and(|n| drawn >= n) {
            break;
        }
        // The period is `interval` from the start of a draw, as far as the reads allow.
        let elapsed = (env.monotonic)().saturating_sub(started_at);
        (env.sleep)(interval.saturating_sub(elapsed));
    }
    Ok(())
}

/// How a frame reaches the output.
enum Mode {
    /// A pipe or a file: the sections as they are.
    Plain,
    /// The first `--watch` frame: clear the screen, then plain.
    Clear,
    /// Later `--watch` frames: cursor home, each line ended by erase-to-end-of-line, then
    /// erase-to-end-of-screen after the last, so the previous frame shows until overwritten.
    InPlace,
}

struct Frame<'a> {
    out: &'a mut dyn Write,
    mode: Mode,
}

impl Frame<'_> {
    fn start(&mut self) -> Result<()> {
        match self.mode {
            Mode::Plain => {}
            Mode::Clear => write!(self.out, "{CLEAR_SCREEN}")?,
            Mode::InPlace => write!(self.out, "{CURSOR_HOME}")?,
        }
        Ok(())
    }

    /// Writes one section and flushes it, so it shows before the next read.
    fn section(&mut self, text: &str) -> Result<()> {
        match self.mode {
            Mode::Plain | Mode::Clear => write!(self.out, "{text}")?,
            Mode::InPlace => write!(self.out, "{}", text.replace('\n', "\x1B[K\n"))?,
        }
        self.out.flush()?;
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        if matches!(self.mode, Mode::InPlace) {
            write!(self.out, "{ERASE_BELOW}")?;
        }
        self.out.flush()?;
        Ok(())
    }
}

/// The Loop section's state at `now`: the lock and the task records, and GitHub's queue and
/// awaiting PRs whether or not a loop runs, at most once a minute (`cache`). Only the running
/// task and the tasks done need a live lock.
fn loop_view(
    base: &Path,
    probe: &dyn ProcessProbe,
    source: &LoopSource,
    cloud_slots: Option<u32>,
    cache: &mut looping::Cache,
    now: u64,
) -> looping::View {
    let (lock, file) = looping::read_lock_file(base, probe);
    let work = match (&lock, &file) {
        (looping::Lock::Running(_), Some(file)) => {
            Some(looping::work_for(base, file).unwrap_or_default())
        }
        _ => None,
    };
    // A loop's `--issue` list has its own issues to check, not the queue's (issue 536).
    let pending = work
        .as_ref()
        .and_then(|w| w.listed.as_ref())
        .map(|l| l.pending.clone());
    let snapshot = cache
        .get(now, || {
            looping::fetch_checking(
                source.github,
                &source.repo,
                base,
                pending.as_deref(),
                cloud_slots,
            )
        })
        .clone();
    looping::View {
        lock,
        work,
        snapshot: Some(snapshot),
        pause: crate::task::loop_pause::read(base),
        jobs: looping::read_jobs(base),
    }
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
