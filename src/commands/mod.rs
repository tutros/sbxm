pub mod config_init;
pub mod config_profiles_dir;
pub mod config_show;
pub mod doctor;
pub mod list;
pub mod new;
pub mod open;
pub mod rm;
pub mod run;
pub mod run_init;
pub mod run_show;
pub mod stop;
pub mod task_file_findings;
pub mod task_finish;
pub mod task_gates;
pub mod task_init;
pub mod task_log;
pub mod task_review;
pub mod task_rm;
pub mod task_run;
pub mod task_start;
pub mod task_states;
pub mod task_status;

use std::io::Write;
use std::path::Path;

use anyhow::Result;

use crate::config::Profile;
use crate::harness::Harness;

/// What to check for an invalid generated kit: the profile's `profile.toml`,
/// plus the project's `sandbox.toml` too when `metadata_dir` has one
/// (decisions from #4/#6), used identically by `new` and `doctor`.
pub(crate) fn invalid_kit_check(
    profiles_dir: &Path,
    profile_name: &str,
    metadata_dir: Option<&Path>,
) -> String {
    let profile_toml = profiles_dir.join(profile_name).join("profile.toml");
    let mut checked = format!("profile '{profile_name}' ({})", profile_toml.display());
    if let Some(metadata_dir) = metadata_dir {
        let sandbox_toml = metadata_dir.join("sandbox.toml");
        if sandbox_toml.is_file() {
            checked.push_str(&format!(
                " and the project's sandbox.toml ({})",
                sandbox_toml.display()
            ));
        }
    }
    checked
}

/// The `--harness` argument selecting `harness` for a `sbxm new` hint, empty
/// for the default: `new` keeps its default per decision 135, so a hint
/// suggesting it stays valid without the flag.
fn harness_flag(harness: Harness) -> String {
    if harness == Harness::default() {
        String::new()
    } else {
        format!(" --harness {}", harness.as_str())
    }
}

/// `harness` followed by a space, empty for the default, for the "no sandbox
/// at all" messages that pair with `harness_flag`'s `sbxm new` hint above.
fn harness_label(harness: Harness) -> String {
    if harness == Harness::default() {
        String::new()
    } else {
        format!("{} ", harness.as_str())
    }
}

/// The `--harness` argument selecting `harness` for a `sbxm open` hint,
/// always explicit: decision 135 makes `--harness` required on `open`, so a
/// hint that omits it for the default (claude) would be unusable as shown.
fn open_harness_flag(harness: Harness) -> String {
    format!(" --harness {}", harness.as_str())
}

/// `harness` followed by a space, always explicit: decision 135 requires
/// `stop`/`rm` to always name the harness they were asked for, so a "no
/// sandbox for X, but Y has one" message can't hide X while naming Y.
fn requested_harness_label(harness: Harness) -> String {
    format!("{} ", harness.as_str())
}

/// Warns about every setting in `profile` that `harness` can't apply in
/// `sandbox`, shared by `new` and `open` so the message and ordering can't
/// diverge (decisions 11, 78).
fn write_unsupported_warnings(
    harness: Harness,
    profile: &Profile,
    sandbox: &str,
    warn: &mut dyn Write,
) -> Result<()> {
    for warning in harness.unsupported(profile, sandbox) {
        writeln!(warn, "warning: {warning}")?;
    }
    Ok(())
}
