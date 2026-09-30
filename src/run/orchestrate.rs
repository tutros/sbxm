//! Runs a comparison's pairs: one throwaway sandbox per (contestant, repeat
//! index) pair (decision 95). Contestants of a wave run in parallel; repeats
//! are sequential waves, so at most one sandbox per contestant is alive at a
//! time however large `repeat` is (decisions 6, 15). Every sandbox is removed
//! afterwards, including after an error (decision 16). Results are only
//! returned here; writing them to disk is slice 8's job.

use std::path::{Path, PathBuf};
use std::thread;

use super::config::{Contestant, RunConfig};
use super::kits::RunKits;
use crate::backend::{CreateSpec, SandboxBackend};
use crate::headless::{self, HeadlessOpts, HeadlessResult};

/// What became of one (contestant, repeat) pair.
#[derive(Debug)]
pub struct PairOutcome {
    /// Index in the run-config's `[[contestants]]`.
    pub contestant: usize,
    /// Zero-based repeat index (the wave).
    pub repeat: u32,
    pub sandbox: String,
    /// The host folder mounted into the sandbox; kept after the run.
    pub workspace: PathBuf,
    /// `Err` means the pair couldn't run at all (create or exec failed); a
    /// timed-out or failed command is `Ok` with that status and its partial
    /// output (decision 16).
    pub result: Result<HeadlessResult, String>,
    /// The sandbox couldn't be removed; the result is kept.
    pub remove_error: Option<String>,
}

/// Runs every pair, wave by wave, and returns the outcomes ordered by repeat,
/// then contestant. `workspaces_root` is `<base>/runs/<run-id>`; pair
/// workspaces are created under it as `<contestant>/<repeat>`.
pub fn execute(
    backend: &dyn SandboxBackend,
    run_config: &RunConfig,
    kits: &RunKits,
    run_id: &str,
    workspaces_root: &Path,
) -> Vec<PairOutcome> {
    let mut outcomes = Vec::new();
    for repeat in 0..run_config.run.repeat {
        let wave: Vec<PairOutcome> = thread::scope(|scope| {
            let handles: Vec<_> = run_config
                .contestants
                .iter()
                .enumerate()
                .map(|(i, contestant)| {
                    let pair = Pair {
                        backend,
                        run_config,
                        kits,
                        contestant,
                        index: i,
                        repeat,
                        sandbox: format!("sbxm-run-{run_id}-{i}-{repeat}"),
                        workspace: workspaces_root.join(i.to_string()).join(repeat.to_string()),
                    };
                    let (sandbox, workspace) = (pair.sandbox.clone(), pair.workspace.clone());
                    (i, sandbox, workspace, scope.spawn(move || pair.run()))
                })
                .collect();
            // Every pair is joined before the next wave starts.
            handles
                .into_iter()
                .map(|(i, sandbox, workspace, handle)| {
                    handle.join().unwrap_or_else(|_| {
                        let remove_error = remove(backend, &sandbox);
                        PairOutcome {
                            contestant: i,
                            repeat,
                            sandbox,
                            workspace,
                            result: Err("the pair's thread panicked".into()),
                            remove_error,
                        }
                    })
                })
                .collect()
        });
        outcomes.extend(wave);
    }
    outcomes
}

struct Pair<'a> {
    backend: &'a dyn SandboxBackend,
    run_config: &'a RunConfig,
    kits: &'a RunKits,
    contestant: &'a Contestant,
    index: usize,
    repeat: u32,
    sandbox: String,
    workspace: PathBuf,
}

impl Pair<'_> {
    fn run(self) -> PairOutcome {
        let result = self.attempt();
        // Always, so a failed create or exec can't leave a sandbox behind.
        let remove_error = remove(self.backend, &self.sandbox);
        PairOutcome {
            contestant: self.index,
            repeat: self.repeat,
            sandbox: self.sandbox,
            workspace: self.workspace,
            result,
            remove_error,
        }
    }

    fn attempt(&self) -> Result<HeadlessResult, String> {
        let harness = self.contestant.harness;
        let harness_kits = self
            .kits
            .get(harness)
            .ok_or_else(|| format!("no kits were built for {}", harness.as_str()))?;
        std::fs::create_dir_all(&self.workspace)
            .map_err(|e| format!("cannot create workspace {}: {e}", self.workspace.display()))?;
        self.backend
            .create(&CreateSpec {
                name: self.sandbox.clone(),
                agent: harness.agent_arg().into(),
                workspace: self.workspace.clone(),
                cpus: self.kits.resources.cpus,
                memory: self.kits.resources.memory.clone(),
                skills: self.kits.skills_store,
                kits: harness_kits.dirs.clone(),
            })
            .map_err(|e| format!("cannot create sandbox {}: {e:#}", self.sandbox))?;
        let opts = HeadlessOpts {
            model: self.contestant.model.clone(),
            budget_usd: self.run_config.run.budget_usd,
            // Unseeded workspaces aren't git repos (decision 113); seeded ones come with slice 7.
            is_git_repo: false,
        };
        headless::run(
            self.backend,
            &self.sandbox,
            &in_sandbox_path(&self.workspace),
            harness,
            &self.run_config.task.prompt,
            &opts,
            self.run_config.run.timeout,
        )
        .map_err(|e| {
            format!(
                "cannot run the headless command (exec) in {}: {e:#}",
                self.sandbox
            )
        })
    }
}

fn remove(backend: &dyn SandboxBackend, sandbox: &str) -> Option<String> {
    backend
        .remove(sandbox)
        .err()
        .map(|e| format!("cannot remove sandbox {sandbox}: {e:#}; run `sbx rm -f {sandbox}`"))
}

/// Where `sbx` mounts a host workspace inside the sandbox: at its own path in
/// forward-slash form with a lowercase drive letter, so `E:\x\y` is `/e/x/y`.
pub fn in_sandbox_path(host: &Path) -> PathBuf {
    let text = host.to_string_lossy().replace(char::from(92), "/");
    let bytes = text.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        PathBuf::from(format!("/{}{}", text[..1].to_lowercase(), &text[2..]))
    } else {
        PathBuf::from(text)
    }
}
