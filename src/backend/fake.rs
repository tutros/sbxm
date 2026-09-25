use std::cell::RefCell;

use anyhow::{Result, bail};

use super::{CreateSpec, SandboxBackend, SandboxInfo};

/// Records calls instead of running `sbx`. Used by tests.
#[derive(Debug, Default)]
pub struct FakeBackend {
    creates: RefCell<Vec<CreateSpec>>,
    stops: RefCell<Vec<String>>,
    removes: RefCell<Vec<String>>,
    fail_remove: bool,
    log: RefCell<Vec<String>>,
    fail_create: bool,
    sandboxes: Vec<SandboxInfo>,
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

    /// A backend whose `list` returns these sandboxes.
    pub fn with_sandboxes(sandboxes: Vec<SandboxInfo>) -> Self {
        Self {
            sandboxes,
            ..Self::default()
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
        Ok(self.sandboxes.clone())
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
}
