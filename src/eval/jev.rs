//! The Jev evaluator (M2a slice 14a, decision 241; spikes S9 and S12): TypeSafe's System One
//! model scores each contestant on the shared rubric, one candidate per request. The call runs
//! in a throwaway evaluator sandbox as one fixed `curl` command, never an agent (decisions 112,
//! 120); the token reaches it only as a header the `sbx` proxy swaps in, from a custom secret
//! scoped to that sandbox (decision 36).

use anyhow::{Result, bail};

use crate::backend::{SandboxBackend, SecretSource};
use crate::commands::doctor;
use crate::run::config::Jev;

/// The one host the evaluator sandbox can reach, and the one the secret is swapped for.
pub const HOST: &str = "api.typesafe.ai";
/// The env var holding the placeholder inside the evaluator sandbox.
pub const ENV: &str = "JEV_API_KEY";
/// The `sbx` that spike S12 verified `secret set-custom` on (it is experimental).
pub const MIN_SBX_VERSION: &str = "0.46.0";

/// What `sbxm run` checks for `[eval.jev]` before anything is written or created: the `sbx`
/// version, the helper's existence (not run: `sbx` verifies it at `set-custom`), and that no
/// global custom secret for the same env and host exists, which would make `sbx create` fail
/// with `400 invalid custom secrets` (S9). Read-only.
pub fn preflight(jev: &Jev, backend: &dyn SandboxBackend) -> Result<()> {
    let version = backend.version()?;
    if doctor::older(&version, MIN_SBX_VERSION) {
        bail!(
            "eval.jev needs sbx {MIN_SBX_VERSION} or newer (its custom secrets), and this is sbx \
             {version}; upgrade sbx or remove [eval.jev]"
        );
    }
    if let SecretSource::Command(helper) = &jev.token
        && !helper.is_file()
    {
        bail!(
            "eval.jev.token_command {} does not exist; create the helper that prints the token \
             there or fix the path",
            helper.display()
        );
    }
    let clash = backend
        .custom_secrets()?
        .into_iter()
        .find(|s| s.is_global() && s.env == ENV && s.targets.iter().any(|t| t == HOST));
    if let Some(clash) = clash {
        bail!(
            "a global custom secret for {ENV} on {HOST} exists, and it would make the Jev \
             evaluator's sandbox fail to start; remove it with `sbx secret rm --placeholder {} \
             -f`",
            clash.placeholder
        );
    }
    Ok(())
}
