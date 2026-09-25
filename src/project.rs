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
    Ok(())
}

fn invalid(name: &str, reason: &str) -> Result<()> {
    bail!("invalid project name '{name}': {reason}")
}
