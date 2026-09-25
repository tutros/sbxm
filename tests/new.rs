use std::path::{Path, PathBuf};

use sbxm::backend::{CreateSpec, FakeBackend};
use sbxm::commands::new;
use tempfile::TempDir;

/// A temp dir holding `config/config.toml` whose `base_dir` is `base/`.
struct Env {
    tmp: TempDir,
}

impl Env {
    fn new() -> Self {
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

    fn config_dir(&self) -> PathBuf {
        self.tmp.path().join("config")
    }

    fn base_dir(&self) -> PathBuf {
        self.tmp.path().join("base")
    }

    fn run(&self, project: &str, backend: &FakeBackend) -> anyhow::Result<()> {
        new::run(&self.config_dir(), project, backend)
    }
}

fn expected_create(workspace: &Path) -> CreateSpec {
    CreateSpec {
        name: "sbxm-demo-claude".into(),
        agent: "claude".into(),
        workspace: workspace.to_path_buf(),
        cpus: 4,
        memory: "8g".into(),
    }
}

#[test]
fn creates_workspace_and_sandbox() {
    let env = Env::new();
    let backend = FakeBackend::default();

    env.run("demo", &backend).unwrap();

    let workspace = env.base_dir().join("demo");
    assert!(workspace.is_dir());
    assert_eq!(backend.creates(), vec![expected_create(&workspace)]);
}
