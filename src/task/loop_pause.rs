//! The pause `task loop` sets on a red `main` (issue 442, decision 299): `<base>/.sbxm/
//! loop-pause.json`, so a restarted loop still sees it and `sbxm status` shows it. While it is
//! set the loop arms no auto-merge and disarms it on its PRs. Only the user lifts it, with `sbxm
//! task loop --clear-pause`; a file that can't be read counts as a pause.

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
    let path = path(base_dir);
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return State::None,
        Err(e) => return State::Unreadable(format!("cannot read {}: {e}", path.display())),
        Ok(meta) if !meta.file_type().is_file() => {
            return State::Unreadable(format!("{} is not a plain file", path.display()));
        }
        Ok(_) => {}
    }
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) => return State::Unreadable(format!("cannot read {}: {e}", path.display())),
    };
    match serde_json::from_str::<Pause>(&text) {
        Ok(pause) => State::Set(pause),
        Err(e) => State::Unreadable(format!("cannot parse {}: {e}", path.display())),
    }
}

/// Writes the pause: to a temporary file next to it, then renamed over it, so a reader never
/// sees half of one.
pub fn write(base_dir: &Path, pause: &Pause) -> Result<()> {
    let path = path(base_dir);
    let dir = path.parent().expect("the pause file has a parent");
    fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let temp = dir.join("loop-pause.json.tmp");
    let text = serde_json::to_string_pretty(pause)?;
    fs::File::create(&temp)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .with_context(|| format!("cannot write {}", temp.display()))?;
    fs::rename(&temp, &path).with_context(|| format!("cannot write {}", path.display()))
}

/// `task loop --clear-pause`: removes the pause file and says what it removed, or that there
/// was none. Only a plain file is removed; anything else there is refused, untouched.
pub fn clear(base_dir: &Path, out: &mut dyn Write) -> Result<()> {
    let path = path(base_dir);
    let state = read(base_dir);
    match &state {
        State::None => {
            writeln!(out, "there was no pause to clear ({})", path.display())?;
            return Ok(());
        }
        State::Unreadable(_)
            if !fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file()) =>
        {
            bail!(
                "{} is not a plain file, so it isn't removed; look at it and remove it by hand",
                path.display()
            );
        }
        _ => {}
    }
    fs::remove_file(&path).with_context(|| format!("cannot remove {}", path.display()))?;
    match state {
        State::Set(pause) => writeln!(out, "cleared the pause: {}", pause.describe())?,
        _ => writeln!(out, "removed the damaged pause file {}", path.display())?,
    }
    Ok(())
}
