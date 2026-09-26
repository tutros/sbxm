use std::cell::RefCell;

use anyhow::{Result, bail};

use std::path::Path;

use super::{CreateSpec, KitValidation, SandboxBackend, SandboxInfo};

/// Records calls instead of running `sbx`. Used by tests.
#[derive(Debug, Default)]
pub struct FakeBackend {
    creates: RefCell<Vec<CreateSpec>>,
    stops: RefCell<Vec<String>>,
    removes: RefCell<Vec<String>>,
    fail_remove: bool,
    log: RefCell<Vec<String>>,
    invalid_kit: Option<String>,
    fail_create: bool,
    sandboxes: Vec<SandboxInfo>,
    secrets: Vec<String>,
    /// `0.43.0` when `None`.
    version: Option<String>,
    no_sbx: bool,
    fail_list: bool,
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

    pub fn creates(&self) -> Vec<CreateSpec> {
        self.creates.borrow().clone()
    }

    pub fn stops(&self) -> Vec<String> {
        self.stops.borrow().clone()
    }

    pub fn removes(&self) -> Vec<String> {
        self.removes.borrow().clone()
    }

    /// Every state-changing call in order, e.g. `"create sbxm-demo-claude"`.
    pub fn log(&self) -> Vec<String> {
        self.log.borrow().clone()
    }

    fn record(&self, call: &str, name: &str) {
        self.log.borrow_mut().push(format!("{call} {name}"));
    }
}

impl SandboxBackend for FakeBackend {
    fn create(&self, spec: &CreateSpec) -> Result<()> {
        self.creates.borrow_mut().push(spec.clone());
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
        self.stops.borrow_mut().push(name.to_owned());
        self.record("stop", name);
        Ok(())
    }

    fn remove(&self, name: &str) -> Result<()> {
        self.removes.borrow_mut().push(name.to_owned());
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
}
