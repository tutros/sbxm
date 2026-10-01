use std::path::Path;

use anyhow::Result;

use crate::config::GlobalConfig;

/// The folder sbxm reads profiles from, one line, for scripts (decision 136).
/// Loads only the global config, so a broken profile can't get in the way and
/// the config's own validation (unknown keys, `default_harness`) applies. A
/// relative path is printed as written, relative to the current directory; an
/// empty one is the current directory, which is how sbxm joins profile names
/// onto it.
pub fn render(config_dir: &Path) -> Result<String> {
    let config = GlobalConfig::load(config_dir)?;
    let dir = config.profiles_dir();
    if dir.as_os_str().is_empty() {
        Ok(".\n".into())
    } else {
        Ok(format!("{}\n", dir.display()))
    }
}
