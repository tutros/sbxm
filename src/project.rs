use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

/// Keeps `sbxm-<project>-<harness>` well under the 63-character hostname
/// limit (decision 44).
const MAX_NAME_LEN: usize = 40;

/// Checks a project name against decision 33 before anything is touched.
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return invalid(name, "the name must not be empty");
    }
    if name.len() > MAX_NAME_LEN {
        return invalid(name, &format!("use at most {MAX_NAME_LEN} characters"));
    }
    let allowed = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-';
    if !name.chars().all(allowed) {
        return invalid(name, "use only lowercase letters, digits and '-'");
    }
    if name.starts_with('-') {
        return invalid(name, "the name must start with a letter or digit");
    }
    if is_windows_device_name(name) {
        return invalid(name, "it is a reserved Windows device name");
    }
    if name == "default" {
        return invalid(name, "'default' is reserved by sbx");
    }
    Ok(())
}

/// `con`, `prn`, `aux`, `nul`, `com0`–`com9`, `lpt0`–`lpt9`. Names are already
/// lowercase and dot-free, so only exact matches matter.
fn is_windows_device_name(name: &str) -> bool {
    match name {
        "con" | "prn" | "aux" | "nul" => true,
        _ => match name.strip_prefix("com").or(name.strip_prefix("lpt")) {
            Some(n) => n.len() == 1 && n.as_bytes()[0].is_ascii_digit(),
            None => false,
        },
    }
}

fn invalid(name: &str, reason: &str) -> Result<()> {
    bail!("invalid project name '{name}': {reason}")
}

/// `sbxm-<project>-<harness>` (decision 41).
pub fn sandbox_name(project: &str, harness: &str) -> String {
    format!("sbxm-{project}-{harness}")
}

/// `<base>/.sbxm/<project>/`: sbxm's metadata, never mounted (decision 40).
pub fn metadata_dir(base_dir: &Path, project: &str) -> PathBuf {
    base_dir.join(".sbxm").join(project)
}
