use std::collections::VecDeque;
use std::sync::Mutex;

use anyhow::{Result, bail};

use std::path::Path;

use super::{CreateSpec, ExecOutput, ExecSpec, KitValidation, SandboxBackend, SandboxInfo};

/// Records calls instead of running `sbx`. Used by tests.
#[derive(Debug, Default)]
pub struct FakeBackend {
    creates: Mutex<Vec<CreateSpec>>,
    stops: Mutex<Vec<String>>,
    removes: Mutex<Vec<String>>,
    fail_remove: bool,
    log: Mutex<Vec<String>>,
    invalid_kit: Option<String>,
    fail_create: bool,
    sandboxes: Vec<SandboxInfo>,
    secrets: Vec<String>,
    /// `0.43.0` when `None`.
    version: Option<String>,
    no_sbx: bool,
    fail_list: bool,
    execs: Mutex<Vec<(String, ExecSpec)>>,
    exec_outputs: Mutex<VecDeque<ExecOutput>>,
    skills: Option<serde_json::Value>,
}

impl FakeBackend {
    /// A backend whose `create` records the call and then fails.
    pub fn failing_create() -> Self {
        Self {
            fail_create: true,
            ..Self::default()
        }
    }

    /// A backend whose `remove` records the call and then fails.
    pub fn failing_remove() -> Self {
        Self {
            fail_remove: true,
            ..Self::default()
        }
    }

    /// A backend whose `validate_kit` reports every kit invalid with `error`.
    pub fn with_invalid_kit(error: &str) -> Self {
        Self {
            invalid_kit: Some(error.to_owned()),
            ..Self::default()
        }
    }

    /// A backend whose `secret_services` returns these names.
    pub fn with_secrets(names: &[&str]) -> Self {
        Self {
            secrets: names.iter().map(|n| n.to_string()).collect(),
            ..Self::default()
        }
    }

    /// A backend whose `list` returns these sandboxes.
    pub fn with_sandboxes(sandboxes: Vec<SandboxInfo>) -> Self {
        Self {
            sandboxes,
            ..Self::default()
        }
    }

    /// Makes `list` return these sandboxes, on top of another constructor.
    pub fn and_sandboxes(self, sandboxes: Vec<SandboxInfo>) -> Self {
        Self { sandboxes, ..self }
    }

    /// Makes `version` report this `sbx` version.
    pub fn with_version(self, version: &str) -> Self {
        Self {
            version: Some(version.to_owned()),
            ..self
        }
    }

    /// Makes `version` and `list` fail as if `sbx` weren't on PATH.
    pub fn without_sbx(self) -> Self {
        Self {
            no_sbx: true,
            ..self
        }
    }

    /// Makes `list` fail as if the daemon weren't running.
    pub fn failing_list(self) -> Self {
        Self {
            fail_list: true,
            ..self
        }
    }

    /// Makes `exec` return these outputs in order, then empty successes.
    pub fn with_exec_outputs(self, outputs: Vec<ExecOutput>) -> Self {
        Self {
            exec_outputs: Mutex::new(outputs.into()),
            ..self
        }
    }

    /// Makes `skills` return this JSON (`null` otherwise).
    pub fn with_skills(self, skills: serde_json::Value) -> Self {
        Self {
            skills: Some(skills),
            ..self
        }
    }

    /// Every `exec` call: sandbox name and spec.
    pub fn execs(&self) -> Vec<(String, ExecSpec)> {
        self.execs.lock().unwrap().clone()
    }

    pub fn creates(&self) -> Vec<CreateSpec> {
        self.creates.lock().unwrap().clone()
    }

    pub fn stops(&self) -> Vec<String> {
        self.stops.lock().unwrap().clone()
    }

    pub fn removes(&self) -> Vec<String> {
        self.removes.lock().unwrap().clone()
    }

    /// Every state-changing call in order, e.g. `"create sbxm-demo-claude"`.
    pub fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    fn record(&self, call: &str, name: &str) {
        self.log.lock().unwrap().push(format!("{call} {name}"));
    }
}

impl SandboxBackend for FakeBackend {
    fn create(&self, spec: &CreateSpec) -> Result<()> {
        self.creates.lock().unwrap().push(spec.clone());
        self.record("create", &spec.name);
        if self.fail_create {
            bail!("fake create failure");
        }
        Ok(())
    }

    fn list(&self) -> Result<Vec<SandboxInfo>> {
        if self.no_sbx {
            bail!("fake: sbx not found");
        }
        if self.fail_list {
            bail!("fake: daemon not running");
        }
        Ok(self.sandboxes.clone())
    }

    fn version(&self) -> Result<String> {
        if self.no_sbx {
            bail!("fake: sbx not found");
        }
        Ok(self.version.clone().unwrap_or_else(|| "0.43.0".into()))
    }

    fn stop(&self, name: &str) -> Result<()> {
        self.stops.lock().unwrap().push(name.to_owned());
        self.record("stop", name);
        Ok(())
    }

    fn remove(&self, name: &str) -> Result<()> {
        self.removes.lock().unwrap().push(name.to_owned());
        self.record("rm", name);
        if self.fail_remove {
            bail!("fake remove failure");
        }
        Ok(())
    }

    fn attach(&self, name: &str) -> Result<()> {
        self.record("attach", name);
        Ok(())
    }

    fn secret_services(&self) -> Result<Vec<String>> {
        Ok(self.secrets.clone())
    }

    fn validate_kit(&self, dir: &Path) -> Result<KitValidation> {
        self.record("validate", &dir.display().to_string());
        Ok(KitValidation {
            valid: self.invalid_kit.is_none(),
            error: self.invalid_kit.clone(),
            warnings: Vec::new(),
        })
    }

    fn exec(&self, sandbox: &str, spec: &ExecSpec) -> Result<ExecOutput> {
        self.execs
            .lock()
            .unwrap()
            .push((sandbox.to_owned(), spec.clone()));
        self.record("exec", sandbox);
        Ok(self
            .exec_outputs
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(ExecOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: Some(0),
            }))
    }

    fn skills(&self) -> Result<serde_json::Value> {
        self.log.lock().unwrap().push("skills".to_owned());
        Ok(self.skills.clone().unwrap_or(serde_json::Value::Null))
    }
}
