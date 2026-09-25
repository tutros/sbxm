use std::cell::RefCell;

use anyhow::{Result, bail};

use super::{CreateSpec, SandboxBackend};

/// Records calls instead of running `sbx`. Used by tests.
#[derive(Debug, Default)]
pub struct FakeBackend {
    creates: RefCell<Vec<CreateSpec>>,
    fail_create: bool,
}

impl FakeBackend {
    /// A backend whose `create` records the call and then fails.
    pub fn failing_create() -> Self {
        Self {
            fail_create: true,
            ..Self::default()
        }
    }

    pub fn creates(&self) -> Vec<CreateSpec> {
        self.creates.borrow().clone()
    }
}

impl SandboxBackend for FakeBackend {
    fn create(&self, spec: &CreateSpec) -> Result<()> {
        self.creates.borrow_mut().push(spec.clone());
        if self.fail_create {
            bail!("fake create failure");
        }
        Ok(())
    }
}
