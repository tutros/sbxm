use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::config;

const STARTER_PROFILE: &str = r#"description = "Default profile"

# Egress is deny-by-default: list the hosts sandboxes may reach.
[network]
allow = []
deny = []

# Environment variables set inside the sandbox.
[env]

# Secret services that must exist in `sbx secret ls`. Values stay in sbx.
[secrets]
services = []
"#;

pub fn run() -> Result<()> {
    let config_dir = config::config_dir()?;
    let config_path = config_dir.join("config.toml");
    let profiles_dir = config_dir.join("profiles");
    let profile_dir = profiles_dir.join("default");
    let profile_path = profile_dir.join("profile.toml");
    let base_dir = config::home_dir()?.join("sbxm-projects");

    for path in [&config_path, &profile_path] {
        if path.exists() {
            bail!("{} already exists; not overwriting it", path.display());
        }
    }
    fs::create_dir_all(&profile_dir)
        .with_context(|| format!("cannot create {}", profile_dir.display()))?;
    fs::write(&config_path, starter_config(&base_dir, &profiles_dir)?)
        .with_context(|| format!("cannot write {}", config_path.display()))?;
    fs::write(&profile_path, STARTER_PROFILE)
        .with_context(|| format!("cannot write {}", profile_path.display()))?;
    Ok(())
}

fn starter_config(base_dir: &Path, profiles_dir: &Path) -> Result<String> {
    Ok(format!(
        r#"# Projects live in <base_dir>/<project>. Keep it outside %TEMP%/AppData:
# sbx can't mount workspaces there on Windows.
base_dir = {base_dir}
# Profiles are <profiles_dir>/<name>/profile.toml. Keep this directory in git.
profiles_dir = {profiles_dir}
default_profile = "default"
default_harness = "claude"
min_sbx_version = "0.43.0"

# sbx defaults to all CPUs and 16 GiB, so be explicit.
[resources]
cpus = 4
memory = "8g"
"#,
        base_dir = toml_path(base_dir)?,
        profiles_dir = toml_path(profiles_dir)?,
    ))
}

/// A path as a TOML string literal, quoted and escaped.
fn toml_path(path: &Path) -> Result<String> {
    let path = path
        .to_str()
        .with_context(|| format!("path is not valid UTF-8: {}", path.display()))?;
    Ok(toml::Value::String(path.to_owned()).to_string())
}
