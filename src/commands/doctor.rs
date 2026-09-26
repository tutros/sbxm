use std::path::Path;

use crate::backend::SandboxBackend;
use crate::config::GlobalConfig;

/// Every check `doctor` ran, in order.
#[derive(Debug, Default)]
pub struct Report {
    checks: Vec<(bool, String)>,
}

impl Report {
    pub fn failed(&self) -> bool {
        self.checks.iter().any(|(ok, _)| !ok)
    }

    /// One line per check: `ok   <what>` or `FAIL <problem>; <fix>`.
    pub fn render(&self) -> String {
        self.checks
            .iter()
            .map(|(ok, text)| format!("{} {text}\n", if *ok { "ok  " } else { "FAIL" }))
            .collect()
    }

    fn pass(&mut self, text: String) {
        self.checks.push((true, text));
    }

    fn fail(&mut self, text: String) {
        self.checks.push((false, text));
    }
}

/// Checks the setup without changing anything. The config comes first: the
/// other checks depend on it, so they're skipped when it doesn't load.
pub fn run(config_dir: &Path, backend: &dyn SandboxBackend) -> Report {
    let mut report = Report::default();
    let config_path = config_dir.join("config.toml");
    let config = match GlobalConfig::load(config_dir) {
        Ok(config) => config,
        Err(err) => {
            report.fail(format!("config: {err:#}"));
            return report;
        }
    };
    report.pass(format!("config {}", config_path.display()));

    match backend.version() {
        Ok(version) if older(&version, &config.min_sbx_version) => report.fail(format!(
            "sbx {version} is older than min_sbx_version {}; upgrade sbx or lower \
             min_sbx_version in {}",
            config.min_sbx_version,
            config_path.display()
        )),
        Ok(version) => report.pass(format!(
            "sbx {version} (min_sbx_version {})",
            config.min_sbx_version
        )),
        Err(err) => report.fail(format!("sbx version: {err:#}")),
    }

    match backend.list() {
        Ok(_) => report.pass("sbx daemon answers".into()),
        Err(err) => report.fail(format!("sbx daemon: {err:#}")),
    }
    report
}

/// Compares dotted versions numerically (`0.9.0` < `0.43.0`); a part's
/// non-digit suffix (e.g. `-rc1`) is ignored.
fn older(version: &str, min: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> {
        v.split('.')
            .map(|p| {
                let digits: String = p.chars().take_while(char::is_ascii_digit).collect();
                digits.parse().unwrap_or(0)
            })
            .collect()
    };
    parts(version) < parts(min)
}
