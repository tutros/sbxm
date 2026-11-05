//! The disk footer of `sbxm status` (decision 171(b), issue 92): free space on the drive of
//! `base_dir` and on the drive of `sbx`'s sandbox state, the drives decision 194 checks (the state
//! folder found as [`disk::state_dir`] finds it), with a warning under `min_free_disk`. One line
//! when both are on the same drive; a drive whose free space can't be read is a note on its line,
//! never a failure.

use std::path::{Path, PathBuf};

use crate::config::GlobalConfig;
use crate::disk::{self, describe, gib};
use crate::status::tasks::truncate;

/// What the footer reads the drives with, passed in so tests don't depend on the machine (as
/// `doctor`'s `Host` is).
#[derive(Debug, Clone)]
pub struct Disk {
    /// Bytes available to the user on the drive holding the path.
    pub free_space: fn(&Path) -> std::io::Result<u64>,
    /// Whether two paths are on one drive ([`disk::same_drive`]).
    pub same_drive: fn(&Path, &Path) -> bool,
    /// `sandbox_state_dir`, else the platform default; `None` when neither is known.
    pub state_dir: Option<PathBuf>,
    /// Under this many bytes free, a line warns; `0` never warns.
    pub min_free_disk: u64,
}

impl Disk {
    pub fn real(config: &GlobalConfig) -> Self {
        Disk {
            free_space: |path| fs4::available_space(path),
            same_drive: disk::same_drive,
            state_dir: disk::state_dir(config, disk::platform_state_dir().as_deref()),
            min_free_disk: config.min_free_disk,
        }
    }
}

/// What a drive's line is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Holds {
    Base,
    State,
    Both,
}

impl Holds {
    fn label(self) -> &'static str {
        match self {
            Holds::Base => "base_dir",
            Holds::State => "sbx's sandbox state",
            Holds::Both => "base_dir and sbx's sandbox state",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drive {
    /// The folder the free space was read for.
    pub dir: PathBuf,
    pub holds: Holds,
    /// The bytes free, or the first line of why they couldn't be read.
    pub free: Result<u64, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Drive(Drive),
    /// sbxm doesn't know where `sbx` keeps its sandboxes here.
    StateUnknown,
}

/// The footer's lines: the drive of `base`, then the drive of the state folder, or one line for
/// both when they share a drive.
pub fn read(base: &Path, disk: &Disk) -> Vec<Line> {
    let free = |dir: &Path| {
        (disk.free_space)(dir).map_err(|e| e.to_string().lines().next().unwrap_or_default().into())
    };
    let drive = |dir: &Path, holds| {
        Line::Drive(Drive {
            dir: dir.to_owned(),
            holds,
            free: free(dir),
        })
    };
    match &disk.state_dir {
        None => vec![drive(base, Holds::Base), Line::StateUnknown],
        Some(state) if (disk.same_drive)(base, state) => vec![drive(base, Holds::Both)],
        Some(state) => vec![drive(base, Holds::Base), drive(state, Holds::State)],
    }
}

/// The footer, after a blank line, each line cut to `width` on a terminal (`None`: in full).
pub fn render(lines: &[Line], min_free_disk: u64, width: Option<usize>) -> String {
    let mut out = String::from("\n");
    for line in lines {
        let text = match line {
            Line::StateUnknown => "Free space: sbx's sandbox state folder isn't known on this \
                                   platform; set sandbox_state_dir in config.toml to show its \
                                   drive"
                .to_owned(),
            // The verdict and what the drive holds come before its path, so cutting a long path
            // to the terminal's width never cuts the warning (review M-2).
            Line::Drive(drive) => {
                let holds = drive.holds.label();
                let verdict = match &drive.free {
                    Err(why) => format!("couldn't be read for {holds} ({why})"),
                    Ok(free) if *free < min_free_disk => format!(
                        "warning, {} is under min_free_disk {} for {holds}",
                        gib(*free),
                        gib(min_free_disk)
                    ),
                    Ok(free) => format!("{} for {holds}", gib(*free)),
                };
                format!("Free space: {verdict}, on {}", describe(&drive.dir))
            }
        };
        match width {
            Some(width) => out.push_str(&truncate(&text, width)),
            None => out.push_str(&text),
        }
        out.push('\n');
    }
    out
}
