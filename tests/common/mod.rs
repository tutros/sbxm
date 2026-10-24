//! Shared test helpers. Each test crate uses only some of them.
#![allow(dead_code)]

pub mod task_fixture;

use std::path::{Path, PathBuf};

use sbxm::backend::{
    CreateSpec, ExecOutput, ExecSpec, FakeBackend, KitValidation, SandboxBackend, SandboxInfo,
};
use sbxm::commands::new;
use tempfile::TempDir;

/// A temp dir holding `config/config.toml` whose `base_dir` is `base/`.
pub struct Env {
    pub tmp: TempDir,
}

impl Env {
    pub fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let env = Env { tmp };
        std::fs::create_dir_all(env.config_dir()).unwrap();
        std::fs::create_dir_all(env.base_dir()).unwrap();
        let base = toml::Value::String(env.base_dir().to_str().unwrap().to_owned());
        let profiles = toml::Value::String(env.profiles_dir().to_str().unwrap().to_owned());
        std::fs::write(
            env.config_dir().join("config.toml"),
            format!(
                "base_dir = {base}\nprofiles_dir = {profiles}\ndefault_profile = \"default\"\n\n\
                 [resources]\ncpus = 4\nmemory = \"8g\"\n"
            ),
        )
        .unwrap();
        env.write_profile("default", "description = \"test default\"\n");
        env
    }

    pub fn profiles_dir(&self) -> PathBuf {
        self.tmp.path().join("profiles")
    }

    /// Writes `<profiles_dir>/<name>/profile.toml`.
    pub fn write_profile(&self, name: &str, contents: &str) {
        let dir = self.profiles_dir().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("profile.toml"), contents).unwrap();
    }

    /// Writes top-level `lines` into the test config, before its `[resources]` table.
    pub fn set_config(&self, lines: &str) {
        let path = self.config_dir().join("config.toml");
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, format!("{lines}\n{text}")).unwrap();
    }

    pub fn config_dir(&self) -> PathBuf {
        self.tmp.path().join("config")
    }

    pub fn base_dir(&self) -> PathBuf {
        self.tmp.path().join("base")
    }

    pub fn run(&self, project: &str, backend: &FakeBackend) -> anyhow::Result<()> {
        self.run_with(project, &new::Options::default(), backend)
    }

    pub fn run_with(
        &self,
        project: &str,
        options: &new::Options,
        backend: &FakeBackend,
    ) -> anyhow::Result<()> {
        new::run(
            &self.config_dir(),
            project,
            options,
            backend,
            &mut std::io::sink(),
        )
    }

    /// `.sbxm/<project>/kits/<hash-prefix>/common`: the one generated kit.
    pub fn kit_dir(&self, project: &str) -> PathBuf {
        let kits = self.base_dir().join(".sbxm").join(project).join("kits");
        let dirs: Vec<PathBuf> = std::fs::read_dir(&kits)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(dirs.len(), 1, "expected one hash dir in {}", kits.display());
        dirs[0].join("common")
    }

    /// `.sbxm/<project>/kits/<hash-prefix>/harness-claude`, next to `kit_dir`.
    pub fn harness_kit_dir(&self, project: &str) -> PathBuf {
        self.kit_dir(project).with_file_name("harness-claude")
    }

    /// A seed dir with `a.txt` and `sub/b.txt`, outside the base dir.
    pub fn seed(&self) -> PathBuf {
        let seed = self.tmp.path().join("seed");
        std::fs::create_dir_all(seed.join("sub")).unwrap();
        std::fs::write(seed.join("a.txt"), "a").unwrap();
        std::fs::write(seed.join("sub").join("b.txt"), "b").unwrap();
        seed
    }
}

/// Plain `git` for building fixtures; returns trimmed stdout.
/// A [`sbxm::disk::Guard`] that finds 1 GiB free on the drive of an existing sandbox state
/// folder in `env`, under the default threshold of 10 GiB (issue 210).
pub fn low_disk(env: &Env) -> sbxm::disk::Guard {
    let state = env.tmp.path().join("sbx-state");
    std::fs::create_dir_all(&state).unwrap();
    sbxm::disk::Guard {
        free_space: |_| Ok(1024 * 1024 * 1024),
        default_state_dir: Some(state),
        ignore: false,
    }
}

/// The start of the refusal [`low_disk`] leads to.
pub const LOW_DISK: &str = "only 1.0 GiB free on ";

/// `path` as a TOML string.
pub fn toml_path(path: &Path) -> String {
    toml::Value::String(path.to_str().unwrap().to_owned()).to_string()
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    // Windows can briefly refuse git a file an indexer or antivirus holds (issue #39), which
    // shows up as "Permission denied" or "failed to write object"; the production runner
    // retries that, so the fixture does too.
    let mut out = None;
    for attempt in 0..5 {
        let run = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        let failed = !run.status.success();
        if failed {
            // Fixture git commands are deterministic, so a failure is environmental (a file lock
            // under load); say what it was, so the cause can be found, and try again.
            eprintln!(
                "fixture git {args:?} failed on attempt {}: {}",
                attempt + 1,
                String::from_utf8_lossy(&run.stderr).trim()
            );
        }
        out = Some(run);
        if !failed {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100 << attempt));
    }
    let out = out.unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// A local bare repo standing in for GitHub, with `main` holding one commit (a README).
pub fn git_origin(root: &Path) -> PathBuf {
    let origin = root.join("origin.git");
    let seed = root.join("origin-seed");
    std::fs::create_dir_all(&origin).unwrap();
    std::fs::create_dir_all(&seed).unwrap();
    git(&origin, &["init", "--bare", "-b", "main"]);
    git(&seed, &["init", "-b", "main"]);
    std::fs::write(seed.join("README.md"), "hello\n").unwrap();
    git(&seed, &["add", "-A"]);
    git(&seed, &["commit", "-m", "first"]);
    git(&seed, &["push", origin.to_str().unwrap(), "main"]);
    origin
}

/// A directory link that needs no admin rights: a junction on Windows.
pub fn dir_link(link: &Path, target: &Path) {
    #[cfg(windows)]
    {
        let status = std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "mklink /J failed");
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
}

/// A symlink to a file. Unlike `dir_link`'s junction, a Windows file symlink
/// needs admin rights or Developer Mode, so tests using this helper are
/// unix-only.
#[cfg(unix)]
pub fn file_link(link: &Path, target: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

/// While it lives, deleting the folder tree holding `dir` fails, modeling a folder Windows holds
/// open or locked. On unix `dir` is made read-only, so nothing directly inside it can be deleted or
/// renamed away; `None` where that doesn't take (running as root), and the calling test then
/// returns early. On Windows `dir` itself is held open without delete sharing, so `dir` can't be
/// deleted or renamed (its contents still can); a guard that doesn't lock it panics rather than
/// letting the test pass without its assertions.
pub struct Undeletable(#[cfg(unix)] PathBuf, #[cfg(windows)] std::fs::File);

impl Undeletable {
    #[cfg(unix)]
    pub fn new(dir: &Path) -> Option<Self> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let guard = Self(dir.to_owned());
        let probe = dir.join(".probe");
        if std::fs::create_dir(&probe).is_ok() {
            let _ = std::fs::remove_dir(&probe);
            return None;
        }
        Some(guard)
    }

    #[cfg(windows)]
    pub fn new(dir: &Path) -> Option<Self> {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x1;
        const FILE_SHARE_WRITE: u32 = 0x2;
        // Needed to open a folder at all.
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        let handle = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(dir)
            .unwrap();
        let probe = dir.with_file_name(".undeletable-probe");
        if std::fs::rename(dir, &probe).is_ok() {
            std::fs::rename(&probe, dir).unwrap();
            panic!("{} can still be renamed while held open", dir.display());
        }
        Some(Self(handle))
    }
}

impl Drop for Undeletable {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
        }
    }
}

/// A `Write` that fails on every call, modeling a screen that has already stopped taking output
/// (decision 185(e)) before a `--restart` command's own recovery runs.
#[derive(Default)]
pub struct AlwaysFails;

impl std::io::Write for AlwaysFails {
    fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("screen output failed"))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Err(std::io::Error::other("screen output failed"))
    }
}

/// Wraps `inner`, delegating every call but `secret_services`, which blocks a restore (puts a
/// plain file at `blocker`, where a set-aside folder needs to go back) and then reports no stored
/// secrets. `secret_services` is only called from `check_secrets`, which `pipeline::prepare` calls
/// after a `--restart` discard has already moved the old task aside, so this is a way to make the
/// restore itself fail without disturbing the move (issue #223). `skip` passes that many leading
/// calls straight through to `inner` instead: `task run --restart` checks the reviewer's secret
/// before any discard moves anything, and that earlier call must not trip the block.
pub struct SecretCheckBlocksRestore<'a> {
    pub inner: &'a FakeBackend,
    pub blocker: PathBuf,
    pub skip: std::sync::atomic::AtomicU32,
}

impl SandboxBackend for SecretCheckBlocksRestore<'_> {
    fn located(&self, _location: sbxm::backend::Location) -> &dyn SandboxBackend {
        self
    }

    fn create(&self, spec: &CreateSpec) -> anyhow::Result<()> {
        self.inner.create(spec)
    }

    fn copy_in(&self, sandbox: &str, host: &Path, path: &str) -> anyhow::Result<()> {
        self.inner.copy_in(sandbox, host, path)
    }

    fn copy_out(
        &self,
        sandbox: &str,
        path: &str,
        host: &Path,
    ) -> anyhow::Result<sbxm::backend::CopyOut> {
        self.inner.copy_out(sandbox, path, host)
    }

    fn list(&self) -> anyhow::Result<Vec<SandboxInfo>> {
        self.inner.list()
    }

    fn stop(&self, name: &str) -> anyhow::Result<()> {
        self.inner.stop(name)
    }

    fn remove(&self, name: &str) -> anyhow::Result<()> {
        self.inner.remove(name)
    }

    fn attach(&self, name: &str) -> anyhow::Result<()> {
        self.inner.attach(name)
    }

    fn validate_kit(&self, dir: &Path) -> anyhow::Result<KitValidation> {
        self.inner.validate_kit(dir)
    }

    fn secret_services(&self) -> anyhow::Result<Vec<String>> {
        if self
            .skip
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |left| (left > 0).then(|| left - 1),
            )
            .is_ok()
        {
            return self.inner.secret_services();
        }
        std::fs::write(&self.blocker, b"in the way").unwrap();
        Ok(Vec::new())
    }

    fn version(&self) -> anyhow::Result<String> {
        self.inner.version()
    }

    fn exec(&self, sandbox: &str, spec: &ExecSpec) -> anyhow::Result<ExecOutput> {
        self.inner.exec(sandbox, spec)
    }

    fn skills(&self) -> anyhow::Result<serde_json::Value> {
        self.inner.skills()
    }

    fn template_save(&self, sandbox: &str, tag: &str) -> anyhow::Result<()> {
        self.inner.template_save(sandbox, tag)
    }

    fn template_ls(&self) -> anyhow::Result<Vec<sbxm::backend::TemplateImage>> {
        self.inner.template_ls()
    }

    fn template_rm(&self, tag: &str) -> anyhow::Result<()> {
        self.inner.template_rm(tag)
    }

    fn network_rules(&self, sandbox: &str) -> anyhow::Result<Vec<sbxm::backend::NetworkRule>> {
        self.inner.network_rules(sandbox)
    }

    fn custom_secrets(&self) -> anyhow::Result<Vec<sbxm::backend::CustomSecret>> {
        self.inner.custom_secrets()
    }

    fn set_custom_secret(&self, spec: &sbxm::backend::CustomSecretSpec) -> anyhow::Result<()> {
        self.inner.set_custom_secret(spec)
    }

    fn remove_custom_secret(&self, placeholder: &str, sandbox: &str) -> anyhow::Result<()> {
        self.inner.remove_custom_secret(placeholder, sandbox)
    }
}
