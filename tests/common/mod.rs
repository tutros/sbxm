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
        std::fs::write(
            env.config_dir().join("config.toml"),
            format!("base_dir = {base}\n\n[resources]\ncpus = 4\nmemory = \"8g\"\n"),
        )
        .unwrap();
        env
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
        new::run(&self.config_dir(), project, options, backend)
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
