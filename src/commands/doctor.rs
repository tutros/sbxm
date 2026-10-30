use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Result, bail};
use clap::ValueEnum;

use super::new;
use crate::auth::{self, TokenEndpoint};
use crate::backend::{Location, SandboxBackend, TemplateImage};
use crate::config::{self, AntigravityAuth, GlobalConfig, Profile};
use crate::disk::{self, gib};
use crate::harness::Harness;
use crate::run::results::parse_timestamp;
use crate::status::cloud::{self, Cloud, Left};
use crate::status::tasks::format_duration;
use crate::task::config::{self as task_config, TaskConfig};
use crate::task::pipeline::CLOUD_MIN_SBX_VERSION;
use crate::task::record;
use crate::{kit, project, state, template};

const GIB: u64 = 1024 * 1024 * 1024;
const DAY: u64 = 86_400;
/// After this many days a template's frozen agent is worth a rebuild (decision 203(c)).
const MAX_TEMPLATE_DAYS: u64 = 14;
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
    /// Where `sbx` keeps its sandboxes on this platform, when known
    /// ([`disk::platform_state_dir`]); `sandbox_state_dir` overrides it.
    pub sandbox_state_dir: Option<PathBuf>,
    /// Seconds since the Unix epoch, for the age of a template.
    pub now: fn() -> u64,
    /// The folder `doctor` runs in, whose `sbxm-task.toml` says whether it is a cloud repo
    /// (P13 (a), decision 262); `None` when it can't be read.
    pub current_dir: Option<PathBuf>,
    /// Where the Antigravity login is refreshed, checked in login mode (decision 270).
    pub token_endpoint: &'static (dyn TokenEndpoint + Sync),
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
            sandbox_state_dir: disk::platform_state_dir(),
            now: || {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs())
            },
            current_dir: std::env::current_dir().ok(),
            token_endpoint: &auth::HostEndpoint,
        }
    }
}

/// How a check came out. Only `Fail` fails `doctor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Ok,
    Skip,
    Warn,
    Fail,
}

/// Every check `doctor` ran, in order.
#[derive(Debug, Default)]
pub struct Report {
    checks: Vec<(Outcome, String)>,
}

impl Report {
    pub fn failed(&self) -> bool {
        self.checks
            .iter()
            .any(|(outcome, _)| *outcome == Outcome::Fail)
    }

    /// One line per check: `ok   <what>`, `skip <what>; <how to check it>`,
    /// `warn <problem>; <fix>` or `FAIL <problem>; <fix>`.
    pub fn render(&self) -> String {
        self.checks
            .iter()
            .map(|(outcome, text)| {
                let label = match outcome {
                    Outcome::Ok => "ok  ",
                    Outcome::Skip => "skip",
                    Outcome::Warn => "warn",
                    Outcome::Fail => "FAIL",
                };
                format!("{label} {text}\n")
            })
            .collect()
    }

    fn pass(&mut self, text: String) {
        self.checks.push((Outcome::Ok, text));
    }

    fn skip(&mut self, text: String) {
        self.checks.push((Outcome::Skip, text));
    }

    fn warn(&mut self, text: String) {
        self.checks.push((Outcome::Warn, text));
    }

    fn fail(&mut self, text: String) {
        self.checks.push((Outcome::Fail, text));
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

    let sbx_version = backend.version();
    match &sbx_version {
        Ok(version) if older(version, &config.min_sbx_version) => report.fail(format!(
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
    check_cloud(&mut report, backend, host);
    check_cloud_use(
        &mut report,
        &config.base_dir,
        sbx_version.as_ref().ok().map(String::as_str),
        host,
    );

    check_base_dir(&mut report, &config.base_dir, &config_path, host);
    check_state_drive(&mut report, &config, &config_path, host);
    check_configs(&mut report, &config, &config_path, backend);
    check_antigravity_login(&mut report, &config, config_dir, host);
    check_templates(
        &mut report,
        &config,
        sbx_version.ok().as_deref(),
        backend,
        host,
    );
    report
}

/// In login mode, the saved Antigravity login and one refresh, with no sandbox (decision 270,
/// point 5); the refreshed token isn't cached or shown.
fn check_antigravity_login(
    report: &mut Report,
    config: &GlobalConfig,
    config_dir: &Path,
    host: &Host,
) {
    if config.antigravity_auth != AntigravityAuth::Login {
        return;
    }
    let path = auth::login_path(config_dir);
    let checked = host.token_endpoint.check().and_then(|()| {
        auth::check_login(
            config_dir,
            &config.base_dir,
            (host.now)(),
            host.token_endpoint,
        )
    });
    match checked {
        Ok(()) => report.pass(format!("Antigravity login {} refreshes", path.display())),
        Err(err) => report.fail(format!(
            "Antigravity login (antigravity_auth = \"login\"): {err:#}"
        )),
    }
}

/// The running cloud sandboxes named `sbxm-`, one line each (decision 259). A cloud that can't
/// be read only warns (P17 (a)): a user who never uses the cloud must not see `doctor` fail.
fn check_cloud(report: &mut Report, backend: &dyn SandboxBackend, host: &Host) {
    match cloud::read(backend) {
        Cloud::Sandboxes(sandboxes) => {
            let now = (host.now)();
            for sandbox in &sandboxes {
                let left = match cloud::time_left(sandbox, now) {
                    Left::Secs(secs) => format!("{} left", format_duration(secs)),
                    Left::Expiring => "expiring".to_owned(),
                    Left::Unknown => "expiry unknown".to_owned(),
                };
                report.pass(format!(
                    "cloud sandbox {}: {}, {}, {left}",
                    sandbox.name,
                    sandbox.status,
                    cloud::shape(sandbox)
                ));
            }
        }
        Cloud::Unreadable(error) => report.warn(format!(
            "cloud sandboxes {}: {error}; if you use the cloud, log in with `sbx login`",
            cloud::UNREADABLE
        )),
    }
}

/// When the cloud is in use, the `sbx` version it needs (P12), and a cloud repo's sandbox gates
/// that name `/tmp` (P13 (a) with (i), decision 262). The cloud is in use when the current
/// directory's `sbxm-task.toml` says `location = "cloud"` or a task record runs in the cloud. The
/// file is read only for this: one that doesn't load is left to the `task` commands to report,
/// and so is a task record that doesn't load.
fn check_cloud_use(report: &mut Report, base_dir: &Path, sbx_version: Option<&str>, host: &Host) {
    let cloud_repo = host.current_dir.as_deref().and_then(|dir| {
        let path = dir.join(task_config::FILE_NAME);
        let config = TaskConfig::load(dir).ok()?;
        (config.sandbox.location == Location::Cloud).then_some((path, config))
    });
    let reason = match &cloud_repo {
        Some((path, _)) => Some(format!("{} has location = \"cloud\"", path.display())),
        // Each record is read on its own, so one that doesn't load hides no other.
        None => {
            let mut tasks: Vec<_> = record::task_folders(base_dir)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|dir| record::read(&dir.join("task.json")).ok())
                .collect();
            record::sort(&mut tasks);
            tasks
                .into_iter()
                .find(|task| task.location == Location::Cloud)
                .map(|task| format!("task {} runs in the cloud", task.id))
        }
    };
    if let (Some(version), Some(reason)) = (sbx_version, reason)
        && older(version, CLOUD_MIN_SBX_VERSION)
    {
        report.warn(format!(
            "sbx {version} is older than {CLOUD_MIN_SBX_VERSION}, the version a cloud task \
             needs (the cloud is in use: {reason}); upgrade sbx"
        ));
    }
    let Some((path, config)) = cloud_repo else {
        return;
    };
    for gate in config.gates.sandbox.iter().filter(|gate| names_tmp(gate)) {
        report.warn(format!(
            "gate `{gate}` in {} uses /tmp, which is a small tmpfs in a cloud sandbox; build \
             under $HOME instead (e.g. $HOME/target)",
            path.display()
        ));
    }
}

/// Whether a gate command names `/tmp` (P13 (i)): `/tmp/`, or `/tmp` ending a path, followed by
/// the end or a character that can't continue a file name, so `/tmpfoo`, `/tmp2`, `/tmp-x` and
/// `/tmp.d` don't count. What comes before `/tmp` isn't looked at.
fn names_tmp(command: &str) -> bool {
    command.match_indices("/tmp").any(|(at, _)| {
        command[at + "/tmp".len()..]
            .chars()
            .next()
            .is_none_or(|c| c == '/' || !(c.is_alphanumeric() || matches!(c, '_' | '-' | '.')))
    })
}

/// A template is listed with its size and age (decision 205(c)) and warned about when its image
/// is gone, its profile's build part changed or it is over [`MAX_TEMPLATE_DAYS`] old (decision
/// 203); only an `sbx` below the template minimum fails, and only when a profile has
/// `[template]` (decision 213).
fn check_templates(
    report: &mut Report,
    config: &GlobalConfig,
    sbx_version: Option<&str>,
    backend: &dyn SandboxBackend,
    host: &Host,
) {
    let profiles_dir = config.profiles_dir();
    let with_template: Vec<String> = profile_names(profiles_dir)
        .unwrap_or_default()
        .into_iter()
        .filter(|name| Profile::load(profiles_dir, name).is_ok_and(|p| p.template.is_some()))
        .collect();
    if let (Some(version), false) = (sbx_version, with_template.is_empty()) {
        if older(version, template::MIN_SBX_VERSION) {
            let quoted: Vec<String> = with_template.iter().map(|n| format!("'{n}'")).collect();
            let (who, them) = match quoted.len() {
                1 => (format!("profile {} has", quoted[0]), "the profile"),
                _ => (
                    format!("profiles {} have", quoted.join(", ")),
                    "those profiles",
                ),
            };
            report.fail(format!(
                "sbx {version} is older than {}, the version templates need ({who} \
                 [template]); upgrade sbx, or remove [template] from {them}",
                template::MIN_SBX_VERSION
            ));
        } else {
            report.pass(format!(
                "sbx {version} (templates need {})",
                template::MIN_SBX_VERSION
            ));
        }
    }

    let records = match template::records(&config.base_dir) {
        Ok(records) if records.is_empty() => return,
        Ok(records) => records,
        Err(err) => {
            report.warn(format!("template records: {err:#}"));
            return;
        }
    };
    let images = match backend.template_ls() {
        Ok(images) => Some(images),
        Err(err) => {
            report.warn(format!(
                "sbx template ls: {err:#}; template images aren't checked"
            ));
            None
        }
    };
    let now = (host.now)();
    for (profile_name, harness, record) in &records {
        let tag = &record.tag;
        let age = parse_timestamp(&record.created_at).map(|at| now.saturating_sub(at));
        report.pass(format!(
            "template {tag} ({profile_name}, {}, {}): {}, {} old",
            harness.as_str(),
            if record.warm { "warm" } else { "cold" },
            gib(record.size_bytes),
            age.map_or("?".to_owned(), format_duration)
        ));
        let flag = match harness {
            Harness::Claude => String::new(),
            other => format!(" --harness {}", other.as_str()),
        };
        let warm = if record.warm { " --warm <repo>" } else { "" };
        let rebuild = format!("rebuild it with `sbxm template build {profile_name}{flag}{warm}`");
        if images
            .as_ref()
            .is_some_and(|images| !images.iter().any(|t| t.reference() == *tag))
        {
            report.warn(format!(
                "template {tag}: image missing; rebuild it with `sbxm template build \
                 {profile_name}{flag}{warm}` or remove the record with `sbxm template rm \
                 {profile_name}{flag}`"
            ));
        }
        match Profile::load(profiles_dir, profile_name) {
            Err(err) => report.warn(format!(
                "template {tag}: profile '{profile_name}' doesn't load, so sbxm can't tell \
                 whether the template is stale ({err:#})"
            )),
            Ok(profile) if profile.template.is_none() => report.warn(format!(
                "template {tag}: profile '{profile_name}' no longer has [template]; remove the \
                 template with `sbxm template rm {profile_name}{flag}`"
            )),
            Ok(profile)
                if kit::template_hash(&profile, *harness, record.warm) != record.template_hash =>
            {
                report.warn(format!(
                    "template {tag} is stale: profile '{profile_name}' changed its install steps, \
                     build allow list or harness kit since it was built; {rebuild}"
                ))
            }
            Ok(_) => {}
        }
        if let Some(age) = age.filter(|age| *age > MAX_TEMPLATE_DAYS * DAY) {
            report.warn(format!(
                "template {tag} is {} old, and its agent is frozen at the version it was saved \
                 with; {rebuild}",
                format_duration(age)
            ));
        }
    }
    let templates = format!(
        "{} template{}",
        records.len(),
        if records.len() == 1 { "" } else { "s" }
    );
    match &images {
        Some(images) => {
            let (count, total) = owned_images(&records, images);
            report.pass(format!(
                "{templates} ({count} image{}), {} in total",
                if count == 1 { "" } else { "s" },
                gib(total)
            ));
        }
        None => {
            let total = records.iter().map(|(_, _, r)| r.size_bytes).sum();
            report.pass(format!(
                "{templates}, {} in total (previous images not counted: sbx template ls failed)",
                gib(total)
            ));
        }
    }
}

/// The distinct tags the records name, current and previous, that `sbx template ls` still
/// reports, and the sum of their sizes as `sbx` reports them: a missing image takes no disk.
fn owned_images(
    records: &[(String, Harness, template::Record)],
    images: &[TemplateImage],
) -> (usize, u64) {
    let tags: BTreeSet<&str> = records
        .iter()
        .flat_map(|(_, _, r)| std::iter::once(&r.tag).chain(&r.previous))
        .map(String::as_str)
        .collect();
    tags.iter()
        .filter_map(|tag| images.iter().find(|i| i.reference() == *tag))
        .fold((0, 0), |(count, total), image| {
            (count + 1, total + image.size)
        })
}

/// The folders in `profiles_dir`, sorted; `None` when it can't be read.
fn profile_names(profiles_dir: &Path) -> Option<Vec<String>> {
    let mut names: Vec<String> = fs::read_dir(profiles_dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    Some(names)
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
    let names = profile_names(profiles_dir).unwrap_or_else(|| {
        report.fail(format!(
            "profiles dir {} does not exist; run `sbxm config init` or change profiles_dir in {}",
            profiles_dir.display(),
            config_path.display()
        ));
        Vec::new()
    });
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
    kits: [(String, kit::Spec); 2],
    backend: &dyn SandboxBackend,
    profiles_dir: &Path,
    profile_name: &str,
    metadata_dir: Option<&Path>,
) -> Result<()> {
    for (name, spec) in kits {
        let dir = root.join(&name);
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

/// The drive of `sbx`'s sandbox state only warns: the commands that create a
/// sandbox refuse on it themselves (decision 194).
fn check_state_drive(report: &mut Report, config: &GlobalConfig, config_path: &Path, host: &Host) {
    if config.min_free_disk == 0 {
        report.pass(format!(
            "sandbox drive not checked: min_free_disk is \"0\" in {}",
            config_path.display()
        ));
        return;
    }
    let Some(dir) = disk::state_dir(config, host.sandbox_state_dir.as_deref()) else {
        report.skip(format!(
            "sandbox drive not checked: sbxm knows where sbx keeps its sandboxes only on \
             Windows; set sandbox_state_dir in {} to check it",
            config_path.display()
        ));
        return;
    };
    if !dir.is_dir() {
        report.warn(format!(
            "sbx's sandbox state folder {} does not exist, so its drive isn't checked; check \
             sandbox_state_dir in {}, or that sbx has run on this machine",
            dir.display(),
            config_path.display()
        ));
        return;
    }
    let drive = disk::describe(&dir);
    match (host.free_space)(&dir) {
        Ok(bytes) if bytes < config.min_free_disk => report.warn(format!(
            "only {} free on {drive} (sbx's sandbox state); free up space to at least {} \
             (a full drive makes sbx answer \"try: sbx login\"), or change min_free_disk in {}",
            gib(bytes),
            gib(config.min_free_disk),
            config_path.display()
        )),
        Ok(bytes) => report.pass(format!(
            "{} free on {drive} (sbx's sandbox state)",
            gib(bytes)
        )),
        Err(err) => report.warn(format!(
            "free space of {drive} (sbx's sandbox state): {err}; check sandbox_state_dir in {}",
            config_path.display()
        )),
    }
}

/// Compares dotted versions numerically (`0.9.0` < `0.43.0`); a part's
/// non-digit suffix (e.g. `-rc1`) is ignored.
pub(crate) fn older(version: &str, min: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use super::names_tmp;

    #[test]
    fn tmp_counts_only_as_a_whole_path_part() {
        for command in [
            "CARGO_TARGET_DIR=/tmp/target cargo test",
            "cd /tmp",
            "cd /tmp && cargo build",
            "cd /tmp; ls",
            "TMPDIR='/tmp' cargo test",
            "/tmp",
        ] {
            assert!(names_tmp(command), "{command}");
        }
        for command in [
            "ls /tmpfoo",
            "ls /var/tmp2",
            "ls /tmp-x",
            "ls /tmp.d",
            "ls /tmp_x",
            "CARGO_TARGET_DIR=$HOME/target cargo build",
            "cargo test",
        ] {
            assert!(!names_tmp(command), "{command}");
        }
    }
}
