//! Run IDs: `<date>-<6 lowercase hex>`, e.g. `2026-09-29-a1b2c3` (P2). Sortable
//! by date, short enough to type. A run owns two roots, reserved together
//! with create-dir-that-must-not-exist, so a collision never touches files:
//! `<base>/.sbxm/runs/<id>/` (metadata and results) and `<base>/runs/<id>/`
//! (the mounted workspaces).

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

/// How many random ids [`reserve`] tries before giving up.
const ATTEMPTS: usize = 5;

/// The two directories of a reserved run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRoots {
    pub id: String,
    /// `<base>/.sbxm/runs/<id>`
    pub meta: PathBuf,
    /// `<base>/runs/<id>`
    pub workspaces: PathBuf,
}

/// A fresh id for today (UTC).
pub fn generate() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after 1970");
    let mut hasher = Sha256::new();
    hasher.update(now.as_nanos().to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    hasher.update(COUNTER.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    let digest = hasher.finalize();
    format!(
        "{}-{:02x}{:02x}{:02x}",
        date_from_days(now.as_secs() / 86_400),
        digest[0],
        digest[1],
        digest[2]
    )
}

/// `YYYY-MM-DD` for a day count since 1970-01-01 (proleptic Gregorian,
/// Howard Hinnant's `civil_from_days`).
pub fn date_from_days(days: u64) -> String {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Whether `id` has exactly the generated format, so it can be used in a
/// path without checking for separators or traversal.
pub fn is_valid(id: &str) -> bool {
    let bytes = id.as_bytes();
    bytes.len() == 17
        && bytes.iter().enumerate().all(|(i, b)| match i {
            4 | 7 | 10 => *b == b'-',
            11.. => b.is_ascii_digit() || (b'a'..=b'f').contains(b),
            _ => b.is_ascii_digit(),
        })
}

/// Reserves the roots of a new run under `base_dir` with a random id.
pub fn reserve(base_dir: &Path) -> Result<RunRoots> {
    reserve_with(base_dir, (0..ATTEMPTS).map(|_| generate()))
}

/// Tries each candidate id in turn; one whose meta or workspace root already
/// exists is skipped, and nothing existing is touched.
pub fn reserve_with(
    base_dir: &Path,
    candidates: impl IntoIterator<Item = String>,
) -> Result<RunRoots> {
    let meta_parent = base_dir.join(".sbxm").join("runs");
    let workspaces_parent = base_dir.join("runs");
    for parent in [&meta_parent, &workspaces_parent] {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    for id in candidates {
        assert!(is_valid(&id), "not a run id: {id}");
        let meta = meta_parent.join(&id);
        let workspaces = workspaces_parent.join(&id);
        if !create_new(&meta)? {
            continue;
        }
        if !create_new(&workspaces)? {
            // Only the empty directory this call just made.
            let _ = fs::remove_dir(&meta);
            continue;
        }
        return Ok(RunRoots {
            id,
            meta,
            workspaces,
        });
    }
    bail!(
        "could not reserve a run id under {}; delete stale run folders or try again",
        base_dir.display()
    )
}

/// `true` if the directory was created, `false` if it already existed.
fn create_new(dir: &Path) -> Result<bool> {
    match fs::create_dir(dir) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e).with_context(|| format!("cannot create {}", dir.display())),
    }
}
