//! The pause `task loop` sets on a red `main` (issue 442, decision 299): `<base>/.sbxm/
//! loop-pause.json`, so a restarted loop still sees it and `sbxm status` shows it. While it is
//! set the loop arms no auto-merge and disarms it on its PRs. Only the user lifts it, with `sbxm
//! task loop --clear-pause`; a file that can't be read counts as a pause.
//!
//! Next to it, the medium pause (issue 550, decision 316(4)): `<base>/.sbxm/
//! loop-pause-medium.json`, set by `task loop --pause-medium "<why>"` for a break after a medium
//! auto-merge the loop can't see. It stops the arming of medium-risk PRs only, and the same
//! `--clear-pause` lifts it.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// What a pause records: the red commit, its failed run, who merged it and when it was set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pause {
    pub commit: String,
    pub run_id: u64,
    pub run_url: String,
    /// The failed run's workflow.
    #[serde(default)]
    pub workflow: String,
    pub merged_by: String,
    /// When the loop set it, as `run::results::format_timestamp` writes it.
    pub at: String,
}

impl Pause {
    /// One line naming the commit, the run and who merged it.
    pub fn describe(&self) -> String {
        let workflow = if self.workflow.is_empty() {
            String::new()
        } else {
            format!("{} ", self.workflow)
        };
        format!(
            "main's commit {} failed {workflow}again after a re-run ({}), merged by {}, since \
             {}",
            self.commit, self.run_url, self.merged_by, self.at
        )
    }
}

/// What the pause file says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    None,
    Set(Pause),
    /// Present but not readable as a pause: counted as paused. Why, naming the file.
    Unreadable(String),
}

impl State {
    pub fn paused(&self) -> bool {
        !matches!(self, Self::None)
    }
}

/// `<base>/.sbxm/loop-pause.json`.
pub fn path(base_dir: &Path) -> PathBuf {
    base_dir.join(".sbxm").join("loop-pause.json")
}

/// Reads the pause file; anything there that isn't a readable pause is [`State::Unreadable`].
pub fn read(base_dir: &Path) -> State {
    match read_file::<Pause>(&path(base_dir)) {
        Ok(None) => State::None,
        Ok(Some(pause)) => State::Set(pause),
        Err(why) => State::Unreadable(why),
    }
}

/// Writes the pause: to a temporary file next to it, then renamed over it, so a reader never
/// sees half of one.
pub fn write(base_dir: &Path, pause: &Pause) -> Result<()> {
    write_file(&path(base_dir), pause)
}

/// What the medium pause records (issue 550): the break the user named, and when it was set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediumPause {
    pub why: String,
    /// When it was set, as `run::results::format_timestamp` writes it.
    pub at: String,
}

/// What the medium pause file says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediumState {
    None,
    Set(MediumPause),
    /// Present but not readable as a pause: counted as paused. Why, naming the file.
    Unreadable(String),
}

impl MediumState {
    pub fn paused(&self) -> bool {
        !matches!(self, Self::None)
    }
}

/// `<base>/.sbxm/loop-pause-medium.json`.
pub fn medium_path(base_dir: &Path) -> PathBuf {
    base_dir.join(".sbxm").join("loop-pause-medium.json")
}

/// Reads the medium pause file, as [`read`] reads the other.
pub fn read_medium(base_dir: &Path) -> MediumState {
    match read_file::<MediumPause>(&medium_path(base_dir)) {
        Ok(None) => MediumState::None,
        Ok(Some(pause)) => MediumState::Set(pause),
        Err(why) => MediumState::Unreadable(why),
    }
}

/// Writes the medium pause, as [`write`] writes the other.
pub fn write_medium(base_dir: &Path, pause: &MediumPause) -> Result<()> {
    write_file(&medium_path(base_dir), pause)
}

/// The value in the plain file at `path`; `None` when there is no file, why it can't be read
/// otherwise.
fn read_file<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>, String> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
        Ok(meta) if !meta.file_type().is_file() => {
            return Err(format!("{} is not a plain file", path.display()));
        }
        Ok(_) => {}
    }
    let text =
        fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_str::<T>(&text)
        .map(Some)
        .map_err(|e| format!("cannot parse {}: {e}", path.display()))
}

/// Writes `value` to `path` through a temporary file next to it, renamed over it.
fn write_file<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let dir = path.parent().expect("the pause file has a parent");
    fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temp = dir.join(format!("{name}.tmp"));
    let text = serde_json::to_string_pretty(value)?;
    fs::File::create(&temp)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .with_context(|| format!("cannot write {}", temp.display()))?;
    fs::rename(&temp, path).with_context(|| format!("cannot write {}", path.display()))
}

/// `task loop --clear-pause`: removes the pause file and the medium pause file (issue 550) and
/// says what it removed, or that there was no pause on a red `main`. Only a plain file is
/// removed; anything else at either path is refused before either is removed, untouched.
pub fn clear(base_dir: &Path, out: &mut dyn Write) -> Result<()> {
    let path = path(base_dir);
    let medium = medium_path(base_dir);
    let state = read(base_dir);
    let medium_state = read_medium(base_dir);
    for (unreadable, path) in [
        (matches!(state, State::Unreadable(_)), &path),
        (matches!(medium_state, MediumState::Unreadable(_)), &medium),
    ] {
        if unreadable && !fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file()) {
            bail!(
                "{} is not a plain file, so it isn't removed; look at it and remove it by hand",
                path.display()
            );
        }
    }
    match state {
        State::None => writeln!(out, "there was no pause to clear ({})", path.display())?,
        state => {
            fs::remove_file(&path).with_context(|| format!("cannot remove {}", path.display()))?;
            match state {
                State::Set(pause) => writeln!(out, "cleared the pause: {}", pause.describe())?,
                _ => writeln!(out, "removed the damaged pause file {}", path.display())?,
            }
        }
    }
    match medium_state {
        MediumState::None => {}
        state => {
            fs::remove_file(&medium)
                .with_context(|| format!("cannot remove {}", medium.display()))?;
            match state {
                MediumState::Set(pause) => {
                    writeln!(out, "cleared the medium pause: {}", pause.why)?
                }
                _ => writeln!(
                    out,
                    "removed the damaged medium pause file {}",
                    medium.display()
                )?,
            }
        }
    }
    Ok(())
}
