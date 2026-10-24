//! A task's scoped secrets in the cloud (milestone 4 slice 4; P8, decisions 256, 262(f); S14 Q2;
//! P2 point 6): set before the sandbox's `create`, since secrets attach at create, and removed
//! after the sandbox on every path, since the cloud's `rm` leaves them behind. The cloud's own
//! list is the record of what to remove: `task.json` holds nothing about them. In this milestone
//! no production path has any (P8 (a) refuses every profile secret but the provider logins); the
//! list comes from the command layer, empty outside tests, where a fake value stands in.

use anyhow::{Context, Result};

use super::lifetime::Released;
use crate::backend::{SandboxBackend, ScopedSecret};

/// Stores every one of `secrets` scoped to `sandbox`, in order, before that sandbox is created.
/// Stops at the first failure; the caller then removes whatever landed, from the cloud's list.
pub fn set_all(
    backend: &dyn SandboxBackend,
    sandbox: &str,
    secrets: &[ScopedSecret],
) -> Result<()> {
    for secret in secrets {
        backend
            .set_scoped_secret(sandbox, secret)
            .with_context(|| {
                format!(
                    "cannot store the secret {} for {} scoped to {sandbox}",
                    secret.env, secret.host
                )
            })?;
    }
    Ok(())
}

/// Removes every secret the cloud lists as scoped to `sandbox`, once the sandbox is gone or its
/// removal has been given up on: one line per removed secret and one warning per failure, each
/// warning naming the command that finishes the job by hand. Never fails; the caller shows what
/// it says. Only the service names and the sandbox name are ever mentioned.
pub fn remove_all(backend: &dyn SandboxBackend, sandbox: &str) -> Vec<Released> {
    let names = match backend.scoped_secrets(sandbox) {
        Ok(names) => names,
        Err(e) => {
            return vec![Released::Warning(format!(
                "the cloud's secret list couldn't be read, so any secret scoped to {sandbox} is \
                 still there: {e:#}; list them with `sbx --cloud secret ls` and remove each with \
                 `sbx --cloud secret rm <name> --sandbox {sandbox} -f`"
            ))];
        }
    };
    names
        .into_iter()
        .map(|name| match backend.remove_secret(&name, sandbox) {
            Ok(()) => Released::Line(format!("scoped secret {name} of {sandbox} removed")),
            Err(e) => Released::Warning(format!(
                "scoped secret {name} of {sandbox} could not be removed: {e:#}; remove it with \
                 `sbx --cloud secret rm {name} --sandbox {sandbox} -f`"
            )),
        })
        .collect()
}
