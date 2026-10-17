//! Prebuilt `sbx` templates (milestone 3): which harnesses support them and the `sbx` version
//! they need.

use anyhow::{Result, bail};

use crate::harness::Harness;

/// The `sbx` version templates were tested with (spike S11); older is unsupported (decision 213).
/// The global `min_sbx_version` stays for everything else.
pub const MIN_SBX_VERSION: &str = "0.46.0";

/// `claude` and `codex` (decisions 204, 217) and `pi` (spike S15); the rest are refused.
pub fn check_supported(harness: Harness) -> Result<()> {
    if !matches!(harness, Harness::Claude | Harness::Codex | Harness::Pi) {
        bail!(
            "templates are not supported for the {} harness yet; build one for claude, codex or pi",
            harness.as_str()
        );
    }
    Ok(())
}

/// Refuses an `sbx` older than [`MIN_SBX_VERSION`].
pub fn check_sbx_version(version: &str) -> Result<()> {
    if crate::commands::doctor::older(version, MIN_SBX_VERSION) {
        bail!(
            "sbx {version} is older than {MIN_SBX_VERSION}, the version templates need; upgrade sbx"
        );
    }
    Ok(())
}
