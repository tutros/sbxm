//! Runs a comparison's pairs: one throwaway sandbox per (contestant, repeat
//! index) pair (decision 95). Contestants of a wave run in parallel; repeats
//! are sequential waves, so at most one sandbox per contestant is alive at a
//! time however large `repeat` is (decisions 6, 15). Every sandbox that was
//! asked for is removed afterwards, including after an error (decision 16).
//! Each pair's workspace is seeded first when the run has a seed (decision
//! 14) and its diff is captured before the sandbox goes. Results are only
//! returned here; writing them to disk is slice 8's job.

use std::path::{Path, PathBuf};
use std::thread;

use super::checks::{self, CheckOutcome};
use super::config::{Contestant, RunConfig};
use super::diff::{self, Diff};
use super::id::RunRoots;
use super::kits::RunKits;
use crate::backend::{CreateSpec, SandboxBackend};
use crate::headless::{self, HeadlessOpts, HeadlessResult};
use crate::seed;

/// What became of one (contestant, repeat) pair.
#[derive(Debug)]
pub struct PairOutcome {
    /// Index in the run-config's `[[contestants]]`.
    pub contestant: usize,
    /// Zero-based repeat index (the wave).
    pub repeat: u32,
    /// The profile the pair's kits were built from: the contestant's own or the run's.
    pub profile: String,
    pub sandbox: String,
    /// The host folder mounted into the sandbox; kept after the run.
    pub workspace: PathBuf,
    /// `Err` means the pair couldn't run at all (seeding, create or exec
    /// failed); a timed-out or failed command is `Ok` with that status and its
    /// partial output (decision 16).
    pub result: Result<HeadlessResult, String>,
    /// What the contestant changed in its workspace. `None` when the pair
    /// never got a sandbox (seeding or create failed); `Some(Err)` when the
    /// diff itself couldn't be captured, which doesn't discard `result`.
    pub diff: Option<Result<Diff, String>>,
    /// The `[[eval.checks]]` verdicts, in config order; empty when there are
    /// none or the pair never got a sandbox to run them in.
    pub checks: Vec<CheckOutcome>,
    /// The sandbox couldn't be removed; the result is kept.
    pub remove_error: Option<String>,
    /// The per-pair callback (see [`execute_with`]) failed, e.g. the results
    /// couldn't be written; the result above is still returned.
    pub save_error: Option<String>,
}

/// Called with each pair's outcome the moment that pair is done (from the
/// pair's own thread), before the rest of the wave finishes.
pub type OnPairDone<'a> = &'a (dyn Fn(&PairOutcome) -> Result<(), String> + Sync);

/// [`execute_with`] without a callback.
pub fn execute(
    backend: &dyn SandboxBackend,
    run_config: &RunConfig,
    kits: &RunKits,
    roots: &RunRoots,
) -> Vec<PairOutcome> {
    execute_with(backend, run_config, kits, roots, &|_| Ok(()))
}

/// Runs every pair, wave by wave, and returns the outcomes ordered by repeat,
/// then contestant. Pair workspaces are `<roots.workspaces>/<contestant>/<repeat>`;
/// scratch state (the host's baseline repos) lives under `<roots.meta>/work`.
/// `on_pair_done` runs as soon as a pair finishes, so its results can be
/// saved before any other pair or evaluator has a chance to fail (decision 16).
pub fn execute_with(
    backend: &dyn SandboxBackend,
    run_config: &RunConfig,
    kits: &RunKits,
    roots: &RunRoots,
    on_pair_done: OnPairDone,
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
                        sandbox: format!("sbxm-run-{}-{i}-{repeat}", roots.id),
                        workspace: roots
                            .workspaces
                            .join(i.to_string())
                            .join(repeat.to_string()),
                        work_dir: roots
                            .meta
                            .join("work")
                            .join(i.to_string())
                            .join(repeat.to_string()),
                        on_pair_done,
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
                        let mut outcome = PairOutcome {
                            contestant: i,
                            repeat,
                            profile: effective_profile(&run_config.contestants[i], kits),
                            sandbox,
                            workspace,
                            result: Err("the pair's thread panicked".into()),
                            diff: None,
                            checks: Vec::new(),
                            remove_error,
                            save_error: None,
                        };
                        outcome.save_error = on_pair_done(&outcome).err();
                        outcome
                    })
                })
                .collect()
        });
        outcomes.extend(wave);
    }
    // The host's baseline repos are only needed until each diff is captured.
    let _ = std::fs::remove_dir_all(roots.meta.join("work"));
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
    /// Scratch space for this pair, outside the mounted workspace.
    work_dir: PathBuf,
    on_pair_done: OnPairDone<'a>,
}

/// What `attempt` learned, besides the headless result.
struct Attempt {
    result: Result<HeadlessResult, String>,
    diff: Option<Result<Diff, String>>,
    checks: Vec<CheckOutcome>,
    /// `create` was called, so there may be a sandbox to remove.
    created: bool,
}

impl Pair<'_> {
    fn run(self) -> PairOutcome {
        let attempt = self.attempt();
        // Cleans up a failed create too, but only if create was asked for:
        // removing a sandbox that never existed would only add noise.
        let remove_error = if attempt.created {
            remove(self.backend, &self.sandbox)
        } else {
            None
        };
        let _ = std::fs::remove_dir_all(&self.work_dir);
        let mut outcome = PairOutcome {
            contestant: self.index,
            repeat: self.repeat,
            profile: effective_profile(self.contestant, self.kits),
            sandbox: self.sandbox,
            workspace: self.workspace,
            result: attempt.result,
            diff: attempt.diff,
            checks: attempt.checks,
            remove_error,
            save_error: None,
        };
        outcome.save_error = (self.on_pair_done)(&outcome).err();
        outcome
    }

    fn attempt(&self) -> Attempt {
        let failed = |why: String, created: bool| Attempt {
            result: Err(why),
            diff: None,
            checks: Vec::new(),
            created,
        };
        let harness = self.contestant.harness;
        let profile = effective_profile(self.contestant, self.kits);
        let Some(harness_kits) = self.kits.get_for(&profile, harness) else {
            return failed(
                format!(
                    "no kits were built for {} under profile {profile}",
                    harness.as_str()
                ),
                false,
            );
        };

        // Prepare the workspace: seeded (a fresh repo, one baseline commit) or
        // a plain empty folder (decision 113).
        let git_dir = self.work_dir.join("baseline.git");
        let seed = self.run_config.task.seed.as_deref();
        let baseline = match seed {
            Some(seed) => match seed::seed_contestant(seed, &self.workspace, &git_dir) {
                Ok(id) => Some(id),
                Err(e) => {
                    return failed(
                        format!("cannot seed workspace {}: {e:#}", self.workspace.display()),
                        false,
                    );
                }
            },
            None => {
                if let Err(e) = std::fs::create_dir_all(&self.workspace) {
                    return failed(
                        format!("cannot create workspace {}: {e}", self.workspace.display()),
                        false,
                    );
                }
                None
            }
        };

        if let Err(e) = self.backend.create(&CreateSpec {
            name: self.sandbox.clone(),
            agent: harness.agent_arg().into(),
            workspace: self.workspace.clone(),
            cpus: self.kits.resources.cpus,
            memory: self.kits.resources.memory.clone(),
            skills: harness_kits.skills_store,
            kits: harness_kits.dirs.clone(),
        }) {
            return failed(
                format!("cannot create sandbox {}: {e:#}", self.sandbox),
                true,
            );
        }

        let opts = HeadlessOpts {
            model: self.contestant.model.clone(),
            budget_usd: self.run_config.run.budget_usd,
            is_git_repo: seed.is_some(),
        };
        let result = headless::run(
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
        });

        // The workspace is final whether the command finished, timed out or
        // failed, so its diff is captured in every case (decision 16).
        let diff = match &baseline {
            Some(id) => diff::seeded(&git_dir, &self.workspace, id),
            None => diff::unseeded(&self.workspace),
        }
        .map_err(|e| format!("cannot capture the diff: {e:#}"));

        // Checks come after the diff, so what they build (a `target/`, caches)
        // can't end up in it; they run in the still-alive sandbox, whatever
        // became of the agent's own command (P6).
        let checks = checks::run_checks(
            self.backend,
            &self.sandbox,
            &in_sandbox_path(&self.workspace),
            &self.run_config.eval.checks,
            self.run_config.run.timeout,
        );
        Attempt {
            result,
            diff: Some(diff),
            checks,
            created: true,
        }
    }
}

/// The contestant's own profile, else the run's (decision 115).
fn effective_profile(contestant: &Contestant, kits: &RunKits) -> String {
    contestant
        .profile
        .clone()
        .unwrap_or_else(|| kits.profile_name.clone())
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
