use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Result, bail};
use clap::ValueEnum;

use super::new;
use crate::backend::SandboxBackend;
use crate::config::{self, GlobalConfig, Profile};
use crate::harness::Harness;
use crate::{kit, project, state};

const GIB: u64 = 1024 * 1024 * 1024;
/// Sandbox images and workspaces take several GB each (decision 67).
const MIN_FREE: u64 = 10 * GIB;

/// What `doctor` asks of the machine, passed in so tests don't depend on it.
pub struct Host {
    /// Bytes available to the user on the drive holding the path.
    pub free_space: fn(&Path) -> std::io::Result<u64>,
    /// Succeeds if a file can be created (and removed) in the directory.
    pub writable: fn(&Path) -> std::io::Result<()>,
    /// Directories `sbx` can't mount workspaces under (S1).
    pub temp_dirs: Vec<PathBuf>,
}

impl Host {
    pub fn real() -> Self {
        let mut temp_dirs = vec![std::env::temp_dir()];
        for var in ["TEMP", "TMP", "APPDATA", "LOCALAPPDATA"] {
            if let Some(dir) = std::env::var_os(var) {
                temp_dirs.push(dir.into());
            }
        }
        Host {
            free_space: |path| fs4::available_space(path),
            writable: |dir| {
                let probe = dir.join(format!(".sbxm-doctor-{}", std::process::id()));
                std::fs::write(&probe, b"")?;
                std::fs::remove_file(&probe)
            },
            temp_dirs,
        }
    }
}

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
pub fn run(config_dir: &Path, backend: &dyn SandboxBackend, host: &Host) -> Report {
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

    check_base_dir(&mut report, &config.base_dir, &config_path, host);
    check_configs(&mut report, &config, &config_path, backend);
    report
}

/// Every profile, then every project merged over its recorded profile
/// (decision 66): each must load, have its secrets stored and give valid
/// kits.
fn check_configs(
    report: &mut Report,
    config: &GlobalConfig,
    config_path: &Path,
    backend: &dyn SandboxBackend,
) {
    let profiles_dir = config.profiles_dir();
    let mut names: Vec<String> = match fs::read_dir(profiles_dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => {
            report.fail(format!(
                "profiles dir {} does not exist; run `sbxm config init` or change profiles_dir in {}",
                profiles_dir.display(),
                config_path.display()
            ));
            Vec::new()
        }
    };
    names.sort();
    let mut stored = None;
    for name in names {
        let result = Profile::load(profiles_dir, &name).and_then(|profile| {
            // Until a profile names its harnesses, Claude stands in (decision 71).
            check_sandbox(
                &name,
                &profile,
                Harness::Claude,
                config,
                None,
                backend,
                &mut stored,
            )
        });
        match result {
            Ok(()) => report.pass(format!("profile '{name}'")),
            Err(err) => report.fail(format!("profile '{name}': {err:#}")),
        }
    }

    let states = match state::load_all(&config.base_dir) {
        Ok(states) => states,
        Err(err) => {
            report.fail(format!("project state: {err:#}"));
            return;
        }
    };
    for (name, state) in states {
        for (harness, entry) in state.sandboxes {
            let profile_name = entry
                .profile
                .unwrap_or_else(|| config.default_profile.clone());
            let label = format!("project {name} ({harness}, profile '{profile_name}')");
            let Ok(harness) = Harness::from_str(&harness, false) else {
                report.fail(format!(
                    "{label}: sbxm doesn't know this harness; use an sbxm version that does, \
                     or remove its entry from {}",
                    project::metadata_dir(&config.base_dir, &name)
                        .join("state.json")
                        .display()
                ));
                continue;
            };
            let metadata_dir = project::metadata_dir(&config.base_dir, &name);
            let result = Profile::load(profiles_dir, &profile_name)
                .and_then(|p| p.with_project(&metadata_dir))
                .and_then(|p| {
                    check_sandbox(
                        &profile_name,
                        &p,
                        harness,
                        config,
                        Some(&metadata_dir),
                        backend,
                        &mut stored,
                    )
                });
            match result {
                Ok(()) => report.pass(label),
                Err(err) => report.fail(format!("{label}: {err:#}")),
            }
        }
    }
}

/// Secrets are fetched from `sbx` once, into `stored`. Kits are written to a
/// fresh temp dir for `sbx kit validate`, which is removed afterwards.
fn check_sandbox(
    profile_name: &str,
    profile: &Profile,
    harness: Harness,
    config: &GlobalConfig,
    metadata_dir: Option<&Path>,
    backend: &dyn SandboxBackend,
    stored: &mut Option<Vec<String>>,
) -> Result<()> {
    let services = &profile.secrets.services;
    if !services.is_empty() {
        if stored.is_none() {
            *stored = Some(backend.secret_services()?);
        }
        new::require_secrets(services, stored.as_deref().unwrap_or_default())?;
    }
    static RUN: AtomicUsize = AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "sbxm-doctor-{}-{}",
        std::process::id(),
        RUN.fetch_add(1, Ordering::Relaxed)
    ));
    let hash = config::config_hash(profile_name, profile, &config.resources, harness);
    let result = validate_kits(
        &root,
        kit::all(profile_name, profile, &hash, harness),
        backend,
        config.profiles_dir(),
        profile_name,
        metadata_dir,
    );
    let _ = fs::remove_dir_all(&root);
    result
}

fn validate_kits(
    root: &Path,
    kits: [(&str, kit::Spec); 2],
    backend: &dyn SandboxBackend,
    profiles_dir: &Path,
    profile_name: &str,
    metadata_dir: Option<&Path>,
) -> Result<()> {
    for (name, spec) in kits {
        let dir = root.join(name);
        kit::write(&dir, &spec)?;
        let validation = backend.validate_kit(&dir)?;
        if !validation.valid {
            let checked = super::invalid_kit_check(profiles_dir, profile_name, metadata_dir);
            bail!(
                "generated kit {name} is invalid: {}; check {checked}",
                validation.error.as_deref().unwrap_or("no details from sbx"),
            );
        }
    }
    Ok(())
}

/// Free space is only checked for a base dir that exists.
fn check_base_dir(report: &mut Report, base: &Path, config_path: &Path, host: &Host) {
    if !base.is_dir() {
        report.fail(format!(
            "base dir {} does not exist; create it or change base_dir in {}",
            base.display(),
            config_path.display()
        ));
        return;
    }
    let canonical = std::fs::canonicalize(base).unwrap_or_else(|_| base.to_owned());
    let under = host
        .temp_dirs
        .iter()
        .find(|dir| std::fs::canonicalize(dir).is_ok_and(|dir| canonical.starts_with(dir)));
    let mut ok = true;
    if let Some(dir) = under {
        ok = false;
        report.fail(format!(
            "base dir {} is under {}, where sbx can't mount workspaces; change base_dir",
            base.display(),
            dir.display()
        ));
    }
    if let Err(err) = (host.writable)(base) {
        ok = false;
        report.fail(format!(
            "base dir {} is not writable ({err}); fix its permissions or change base_dir",
            base.display()
        ));
    }
    if ok {
        report.pass(format!("base dir {}", base.display()));
    }

    match (host.free_space)(base) {
        Ok(bytes) if bytes < MIN_FREE => report.fail(format!(
            "only {} free for base dir {}; free up space to at least {} GiB",
            gib(bytes),
            base.display(),
            MIN_FREE / GIB
        )),
        Ok(bytes) => report.pass(format!(
            "{} free for base dir {}",
            gib(bytes),
            base.display()
        )),
        Err(err) => report.fail(format!(
            "free space for base dir: {err}; check that base_dir is on a drive sbx can use"
        )),
    }
}

fn gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / GIB as f64)
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
