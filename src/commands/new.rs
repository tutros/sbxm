use anyhow::{Result, bail};

use crate::project;

pub fn run(name: &str) -> Result<()> {
    project::validate_name(name)?;
    bail!("`new` is not implemented yet")
}
