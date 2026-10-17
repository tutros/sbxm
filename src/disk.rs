//! The free space of the drive holding `sbx`'s sandbox state (issue 210, decision 194). A
//! sandbox's disk file there never shrinks, and a full drive makes `sbx` answer "try: sbx login",
//! so every command that creates a sandbox refuses first when the drive is low.

use std::path::{Component, Path, PathBuf, Prefix};

use anyhow::{Result, bail};

use crate::config::GlobalConfig;

const GIB: u64 = 1024 * 1024 * 1024;

/// `min_free_disk` when `config.toml` doesn't set it.
pub const DEFAULT_MIN_FREE: u64 = 10 * GIB;

/// `<n>k`, `<n>m`, `<n>g` or `<n>t` (powers of 1024, either case), or `0`.
pub fn parse_size(text: &str) -> Result<u64> {
    if text == "0" {
        return Ok(0);
    }
    let unit = text.chars().last().map(|c| c.to_ascii_lowercase());
    let shift = match unit {
        Some('k') => 10,
        Some('m') => 20,
        Some('g') => 30,
        Some('t') => 40,
        _ => bail!("{text:?} has no unit"),
    };
    let digits = &text[..text.len() - 1];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        bail!("{text:?} isn't a whole number with a unit");
    }
    let n: u64 = digits.parse()?;
    n.checked_mul(1 << shift)
        .ok_or_else(|| anyhow::anyhow!("{text:?} is too large"))
}

/// Where `sbx` keeps its sandboxes when `sandbox_state_dir` isn't set: `%LOCALAPPDATA%\
/// DockerSandboxes` on Windows, the only location verified (`sbx` v0.46.0 neither moves nor
/// reports it); unknown elsewhere, where sbxm doesn't guess.
pub fn platform_state_dir() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    std::env::var_os("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join("DockerSandboxes"))
}

/// The state folder a check reads: `sandbox_state_dir`, else the platform default.
pub fn state_dir(config: &GlobalConfig, default: Option<&Path>) -> Option<PathBuf> {
    config
        .sandbox_state_dir
        .clone()
        .or_else(|| default.map(Path::to_owned))
}

/// `drive E: (<dir>)` where the path has a drive, else `the drive of <dir>`.
pub fn describe(dir: &Path) -> String {
    match dir.components().next() {
        Some(Component::Prefix(prefix)) => format!(
            "drive {} ({})",
            prefix.as_os_str().to_string_lossy(),
            dir.display()
        ),
        _ => format!("the drive of {}", dir.display()),
    }
}

pub fn gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / GIB as f64)
}

/// Whether two paths are on the same drive, so a free-space report covering both would count the
/// same space twice (decision 220): the same path always is; otherwise, on Unix, two paths that
/// both exist are compared by the filesystem device they sit on (`stat`'s device id), since two
/// different folders can share a drive without sharing a prefix; everywhere else (a path that
/// doesn't exist yet, or Windows, which has no such device id), a shared drive letter (in either
/// case, plain or `\\?\`) or an identical other prefix is the only other sign of a shared drive.
pub fn same_drive(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    if let (Some(da), Some(db)) = (device_id(a), device_id(b)) {
        return da == db;
    }
    if let (Some(la), Some(lb)) = (drive_letter(a), drive_letter(b)) {
        return la.eq_ignore_ascii_case(&lb);
    }
    matches!(
        (a.components().next(), b.components().next()),
        (Some(Component::Prefix(p1)), Some(Component::Prefix(p2))) if p1.as_os_str() == p2.as_os_str()
    )
}

/// A path's drive letter, for `C:` and `\\?\C:` alike, in whatever case it was written (Windows
/// treats `c:` and `C:` as one drive; PR 264 review M-1).
fn drive_letter(path: &Path) -> Option<u8> {
    match path.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => Some(letter),
            _ => None,
        },
        _ => None,
    }
}

/// The filesystem device a path sits on, or `None` when it can't be read (missing path, or a
/// platform without the concept, like Windows).
#[cfg(unix)]
fn device_id(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| m.dev())
}

#[cfg(not(unix))]
fn device_id(_path: &Path) -> Option<u64> {
    None
}

/// What a sandbox-creating command reads the drive with, passed in so tests don't depend on the
/// machine. The default reads no platform folder, so only `sandbox_state_dir` is checked.
#[derive(Debug, Clone)]
pub struct Guard {
    /// Bytes available to the user on the drive holding the path.
    pub free_space: fn(&Path) -> std::io::Result<u64>,
    /// [`platform_state_dir`] in the real command.
    pub default_state_dir: Option<PathBuf>,
    /// `--ignore-disk`: skip the check for this command.
    pub ignore: bool,
}

impl Default for Guard {
    fn default() -> Self {
        Guard {
            free_space: |path| fs4::available_space(path),
            default_state_dir: None,
            ignore: false,
        }
    }
}

impl Guard {
    pub fn real(ignore: bool) -> Self {
        Guard {
            default_state_dir: platform_state_dir(),
            ignore,
            ..Guard::default()
        }
    }
}

/// Refuses when the state drive has less than `min_free_disk` free. Goes ahead without reading
/// it under `--ignore-disk` or `min_free_disk = "0"`, and when the folder is unknown, missing or
/// its free space can't be read: `doctor` reports those (decision 194).
pub fn require(config: &GlobalConfig, config_dir: &Path, guard: &Guard) -> Result<()> {
    if guard.ignore || config.min_free_disk == 0 {
        return Ok(());
    }
    let Some(dir) = state_dir(config, guard.default_state_dir.as_deref()) else {
        return Ok(());
    };
    if !dir.is_dir() {
        return Ok(());
    }
    match (guard.free_space)(&dir) {
        Ok(free) if free < config.min_free_disk => bail!(
            "only {} free on {} (sbx's sandbox state), under min_free_disk {}; free up space, \
             or pass --ignore-disk to go ahead anyway (min_free_disk in {} sets the threshold, \
             \"0\" switches the check off)",
            gib(free),
            describe(&dir),
            gib(config.min_free_disk),
            config_dir.join("config.toml").display()
        ),
        _ => Ok(()),
    }
}
