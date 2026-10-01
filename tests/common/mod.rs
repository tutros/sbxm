//! Shared test helpers. Each test crate uses only some of them.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use sbxm::backend::FakeBackend;
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
        let stderr = String::from_utf8_lossy(&run.stderr);
        let locked =
            stderr.contains("Permission denied") || stderr.contains("failed to write object");
        out = Some(run);
        if !locked {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50 << attempt));
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
