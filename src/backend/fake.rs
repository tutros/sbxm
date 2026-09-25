use std::cell::RefCell;

use anyhow::Result;

use super::{CreateSpec, SandboxBackend};

/// Records calls instead of running `sbx`. Used by tests.
#[derive(Debug, Default)]
pub struct FakeBackend {
    creates: RefCell<Vec<CreateSpec>>,
}

impl FakeBackend {
    pub fn creates(&self) -> Vec<CreateSpec> {
        self.creates.borrow().clone()
    }
}

impl SandboxBackend for FakeBackend {
    fn create(&self, spec: &CreateSpec) -> Result<()> {
        self.creates.borrow_mut().push(spec.clone());
        Ok(())
    }
}
