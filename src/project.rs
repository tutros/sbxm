use anyhow::{Result, bail};

/// Checks a project name against decision 33 before anything is touched.
pub fn validate_name(name: &str) -> Result<()> {
    let allowed = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-';
    if !name.chars().all(allowed) {
        return invalid(name, "use only lowercase letters, digits and '-'");
    }
    Ok(())
}

fn invalid(name: &str, reason: &str) -> Result<()> {
    bail!("invalid project name '{name}': {reason}")
}
