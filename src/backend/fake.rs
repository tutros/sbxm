use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use anyhow::{Result, bail};

use std::path::Path;

use super::{CreateSpec, ExecOutput, ExecSpec, KitValidation, SandboxBackend, SandboxInfo};

/// Holds every `exec` of a [`FakeBackend`] until [`ExecGate::open`], so a test
/// can look at what exists while runs are "in progress".
#[derive(Debug, Clone, Default)]
pub struct ExecGate {
    inner: Arc<(Mutex<GateState>, Condvar)>,
}

#[derive(Debug, Default)]
struct GateState {
    open: bool,
    blocked: usize,
}

/// A test that waits longer than this has hung; fail it instead.
const GATE_TIMEOUT: Duration = Duration::from_secs(10);

impl ExecGate {
    /// Waits until `n` execs are blocked at the gate.
    pub fn wait_for_blocked(&self, n: usize) {
        let (state, cv) = &*self.inner;
        let (_guard, timeout) = cv
            .wait_timeout_while(state.lock().unwrap(), GATE_TIMEOUT, |s| s.blocked < n)
            .unwrap();
        assert!(
            !timeout.timed_out(),
            "fewer than {n} execs reached the gate"
        );
    }

    /// Lets every blocked and future exec through.
    pub fn open(&self) {
        let (state, cv) = &*self.inner;
        state.lock().unwrap().open = true;
        cv.notify_all();
    }

    fn pass(&self) {
        let (state, cv) = &*self.inner;
        let mut guard = state.lock().unwrap();
        guard.blocked += 1;
        cv.notify_all();
        let (mut guard, timeout) = cv
            .wait_timeout_while(guard, GATE_TIMEOUT, |s| !s.open)
            .unwrap();
        assert!(!timeout.timed_out(), "the exec gate was never opened");
        guard.blocked -= 1;
    }
}

/// Records calls instead of running `sbx`. Used by tests.
#[derive(Debug, Default)]
pub struct FakeBackend {
    creates: Mutex<Vec<CreateSpec>>,
    stops: Mutex<Vec<String>>,
    removes: Mutex<Vec<String>>,
    fail_remove: bool,
    log: Mutex<Vec<String>>,
    invalid_kit: Option<String>,
    fail_create: bool,
    sandboxes: Vec<SandboxInfo>,
    secrets: Vec<String>,
    /// `0.43.0` when `None`.
    version: Option<String>,
    no_sbx: bool,
    fail_list: bool,
    execs: Mutex<Vec<(String, ExecSpec)>>,
    exec_outputs: Mutex<VecDeque<ExecOutput>>,
    skills: Option<serde_json::Value>,
    exec_outputs_for: HashMap<String, ExecOutput>,
    default_exec_output: Option<ExecOutput>,
    fail_create_for: Vec<String>,
    fail_exec_for: Vec<String>,
    gate: Option<ExecGate>,
    /// Only sandboxes whose name ends with this are held at the gate.
    gate_suffix: Option<String>,
    /// Scripted by what the command line contains, in any sandbox.
    exec_outputs_matching: Vec<(String, ExecOutput)>,
    fail_exec_matching: Vec<String>,
    exec_hook: Option<ExecHook>,
    exec_responder: Option<ExecResponder>,
    create_hook: Option<CreateHook>,
    /// How many times `secret_services` was called; a read, so it's not in `log`.
    secret_service_calls: Mutex<usize>,
}

/// Runs inside every `exec` (after the gate), so a test can play the agent
/// by changing files on the host while the "sandbox" is busy.
type HookFn = dyn Fn(&str, &ExecSpec) + Send + Sync;

type CreateHookFn = dyn Fn(&CreateSpec) + Send + Sync;

#[derive(Clone)]
struct CreateHook(Arc<CreateHookFn>);

impl std::fmt::Debug for CreateHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CreateHook")
    }
}

#[derive(Clone)]
struct ExecHook(Arc<HookFn>);

impl std::fmt::Debug for ExecHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ExecHook")
    }
}

/// Answers an `exec` itself (after the hook), or passes with `None` to the scripted outputs.
type ResponderFn = dyn Fn(&str, &ExecSpec) -> Option<ExecOutput> + Send + Sync;

#[derive(Clone)]
struct ExecResponder(Arc<ResponderFn>);

impl std::fmt::Debug for ExecResponder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ExecResponder")
    }
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

    /// Makes `secret_services` return these names, on top of another constructor.
    pub fn and_secrets(self, names: &[&str]) -> Self {
        Self {
            secrets: names.iter().map(|n| n.to_string()).collect(),
            ..self
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

    /// Makes `exec` return these outputs in order, then empty successes.
    pub fn with_exec_outputs(self, outputs: Vec<ExecOutput>) -> Self {
        Self {
            exec_outputs: Mutex::new(outputs.into()),
            ..self
        }
    }

    /// Makes `skills` return this JSON (`null` otherwise).
    pub fn with_skills(self, skills: serde_json::Value) -> Self {
        Self {
            skills: Some(skills),
            ..self
        }
    }

    /// Makes every `exec` in `sandbox` return `output` (not consumed, so it is
    /// independent of the order parallel calls arrive in).
    pub fn with_exec_output_for(mut self, sandbox: &str, output: ExecOutput) -> Self {
        self.exec_outputs_for.insert(sandbox.to_owned(), output);
        self
    }

    /// Makes every `exec` whose argv has an element containing `needle`
    /// return `output`, in any sandbox (before the per-sandbox scripts), e.g.
    /// to give one check command its own exit code.
    pub fn with_exec_output_matching(mut self, needle: &str, output: ExecOutput) -> Self {
        self.exec_outputs_matching.push((needle.to_owned(), output));
        self
    }

    /// Makes every `exec` whose argv contains `needle` record the call and fail.
    pub fn with_failing_exec_matching(mut self, needle: &str) -> Self {
        self.fail_exec_matching.push(needle.to_owned());
        self
    }

    /// What `exec` returns in a sandbox with no other scripted output.
    pub fn with_default_exec_output(self, output: ExecOutput) -> Self {
        Self {
            default_exec_output: Some(output),
            ..self
        }
    }

    /// Makes `create` of this one sandbox record the call and then fail.
    pub fn with_failing_create_for(mut self, sandbox: &str) -> Self {
        self.fail_create_for.push(sandbox.to_owned());
        self
    }

    /// Makes `exec` in this one sandbox record the call and then fail.
    pub fn with_failing_exec_for(mut self, sandbox: &str) -> Self {
        self.fail_exec_for.push(sandbox.to_owned());
        self
    }

    /// Calls `hook(spec)` at the start of every `create`, e.g. to look at what exists on the
    /// host while the sandbox is being made.
    pub fn with_create_hook(self, hook: impl Fn(&CreateSpec) + Send + Sync + 'static) -> Self {
        Self {
            create_hook: Some(CreateHook(Arc::new(hook))),
            ..self
        }
    }

    /// Calls `hook(sandbox, spec)` inside every successful `exec`, before it
    /// returns, e.g. to write files into the workspace like an agent would.
    pub fn with_exec_hook(self, hook: impl Fn(&str, &ExecSpec) + Send + Sync + 'static) -> Self {
        Self {
            exec_hook: Some(ExecHook(Arc::new(hook))),
            ..self
        }
    }

    /// Lets a test compute an `exec`'s output from the command (e.g. run the real `git` it names);
    /// `None` falls through to the scripted outputs.
    pub fn with_exec_responder(
        self,
        responder: impl Fn(&str, &ExecSpec) -> Option<ExecOutput> + Send + Sync + 'static,
    ) -> Self {
        Self {
            exec_responder: Some(ExecResponder(Arc::new(responder))),
            ..self
        }
    }

    /// Like [`FakeBackend::with_exec_gate`], but only holds `exec` in sandboxes
    /// whose name ends with `suffix` (e.g. `-1-0`); the others run freely.
    pub fn with_exec_gate_for(self, suffix: &str) -> (Self, ExecGate) {
        let (fake, gate) = self.with_exec_gate();
        (
            Self {
                gate_suffix: Some(suffix.to_owned()),
                ..fake
            },
            gate,
        )
    }

    /// Holds every `exec` (after recording it) until the returned gate is opened.
    pub fn with_exec_gate(self) -> (Self, ExecGate) {
        let gate = ExecGate::default();
        (
            Self {
                gate: Some(gate.clone()),
                ..self
            },
            gate,
        )
    }

    /// Every `exec` call: sandbox name and spec.
    pub fn execs(&self) -> Vec<(String, ExecSpec)> {
        self.execs.lock().unwrap().clone()
    }

    pub fn creates(&self) -> Vec<CreateSpec> {
        self.creates.lock().unwrap().clone()
    }

    pub fn stops(&self) -> Vec<String> {
        self.stops.lock().unwrap().clone()
    }

    pub fn removes(&self) -> Vec<String> {
        self.removes.lock().unwrap().clone()
    }

    /// Every state-changing call in order, e.g. `"create sbxm-demo-claude"`.
    pub fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    /// How many times `secret_services` was called so far.
    pub fn secret_service_calls(&self) -> usize {
        *self.secret_service_calls.lock().unwrap()
    }

    fn record(&self, call: &str, name: &str) {
        self.log.lock().unwrap().push(format!("{call} {name}"));
    }
}

impl SandboxBackend for FakeBackend {
    fn create(&self, spec: &CreateSpec) -> Result<()> {
        if let Some(hook) = &self.create_hook {
            (hook.0)(spec);
        }
        self.creates.lock().unwrap().push(spec.clone());
        self.record("create", &spec.name);
        if self.fail_create || self.fail_create_for.contains(&spec.name) {
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
        self.stops.lock().unwrap().push(name.to_owned());
        self.record("stop", name);
        Ok(())
    }

    fn remove(&self, name: &str) -> Result<()> {
        self.removes.lock().unwrap().push(name.to_owned());
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
        *self.secret_service_calls.lock().unwrap() += 1;
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

    fn exec(&self, sandbox: &str, spec: &ExecSpec) -> Result<ExecOutput> {
        self.execs
            .lock()
            .unwrap()
            .push((sandbox.to_owned(), spec.clone()));
        self.record("exec", sandbox);
        if let Some(gate) = &self.gate
            && self
                .gate_suffix
                .as_ref()
                .is_none_or(|s| sandbox.ends_with(s))
        {
            gate.pass();
        }
        if self.fail_exec_for.iter().any(|s| s == sandbox) {
            bail!("fake exec failure");
        }
        let mentions = |needle: &String| spec.argv.iter().any(|a| a.contains(needle.as_str()));
        if self.fail_exec_matching.iter().any(mentions) {
            bail!("fake exec failure");
        }
        if let Some(hook) = &self.exec_hook {
            (hook.0)(sandbox, spec);
        }
        if let Some(output) = self
            .exec_responder
            .as_ref()
            .and_then(|r| (r.0)(sandbox, spec))
        {
            return Ok(output);
        }
        if let Some((_, output)) = self.exec_outputs_matching.iter().find(|(n, _)| mentions(n)) {
            return Ok(output.clone());
        }
        if let Some(output) = self.exec_outputs_for.get(sandbox) {
            return Ok(output.clone());
        }
        let queued = self.exec_outputs.lock().unwrap().pop_front();
        Ok(queued
            .or_else(|| self.default_exec_output.clone())
            .unwrap_or(ExecOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: Some(0),
            }))
    }

    fn skills(&self) -> Result<serde_json::Value> {
        self.log.lock().unwrap().push("skills".to_owned());
        Ok(self.skills.clone().unwrap_or(serde_json::Value::Null))
    }
}
