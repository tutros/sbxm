pub mod config_init;
pub mod config_show;
pub mod doctor;
pub mod list;
pub mod new;
pub mod open;
pub mod rm;
pub mod run_init;
pub mod stop;

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

/// The `--harness` argument selecting `harness` in a hint, empty for the
/// default, so Claude-only users see the commands they already know.
fn harness_flag(harness: Harness) -> String {
    if harness == Harness::default() {
        String::new()
    } else {
        format!(" --harness {}", harness.as_str())
    }
}

/// `harness` followed by a space for messages, empty for the default.
fn harness_label(harness: Harness) -> String {
    if harness == Harness::default() {
        String::new()
    } else {
        format!("{} ", harness.as_str())
    }
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
