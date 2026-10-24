use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use anyhow::{Result, bail};

use std::path::{Path, PathBuf};

use super::{
    CopyOut, CreateSpec, CustomSecret, CustomSecretSpec, ExecOutput, ExecSpec, KitValidation,
    Location, NetworkRule, SandboxBackend, SandboxInfo, TemplateImage,
};

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
    /// What the next calls of `remove` fail with, one each, in order; `remove` succeeds once
    /// they are used up.
    remove_errors: Mutex<std::collections::VecDeque<String>>,
    log: Mutex<Vec<String>>,
    invalid_kit: Option<String>,
    kit_warning: Option<String>,
    fail_create: bool,
    fail_stop: bool,
    fail_template_save: bool,
    fail_template_rm: bool,
    /// Only this tag's `template_rm` fails.
    fail_template_rm_for: Option<String>,
    fail_template_ls: bool,
    sandboxes: Vec<SandboxInfo>,
    secrets: Mutex<Vec<String>>,
    /// `0.43.0` when `None`.
    version: Option<String>,
    no_sbx: bool,
    fail_list: bool,
    /// What `list` fails with under `fail_list`, instead of the daemon-not-running text.
    list_failure: Option<String>,
    execs: Mutex<Vec<(String, ExecSpec)>>,
    exec_outputs: Mutex<VecDeque<ExecOutput>>,
    skills: Option<serde_json::Value>,
    exec_outputs_for: HashMap<String, ExecOutput>,
    default_exec_output: Option<ExecOutput>,
    fail_create_for: Vec<String>,
    fail_create_matching: Vec<String>,
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
    /// Flipped (via [`FakeBackend::secret_services_failure`]) to make `secret_services` fail from
    /// then on, modeling `sbx` not answering (issue #201 review M-2).
    fail_secret_services: Arc<std::sync::atomic::AtomicBool>,
    /// What `template_ls` returns; `template_save` adds to it, `template_rm` takes away.
    templates: Mutex<Vec<TemplateImage>>,
    /// What `network_rules` returns, for every sandbox.
    network_rules: Vec<NetworkRule>,
    fail_network_rules: bool,
    /// What `custom_secrets` returns; `set_custom_secret` adds to it, `remove_custom_secret`
    /// takes away.
    custom_secrets: Mutex<Vec<CustomSecret>>,
    /// Every `set_custom_secret` call.
    custom_secret_sets: Mutex<Vec<CustomSecretSpec>>,
    fail_custom_secrets: bool,
    fail_set_custom_secret: bool,
    fail_remove_custom_secret: bool,
    /// Every location `located` was asked for, in order.
    locations: Mutex<Vec<Location>>,
    /// What `copy_out` of an in-sandbox path produces, before `sandbox_root` is looked at.
    copy_outs: HashMap<String, FakeCopy>,
    /// A host folder standing in for every sandbox's filesystem: `copy_in` writes
    /// `<root>/<path>`, `copy_out` copies whatever is at `<root>/<path>` as it is (a link as a
    /// link, a folder whole).
    sandbox_root: Option<PathBuf>,
}

/// What a scripted [`FakeBackend`] `copy_out` leaves at the host path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FakeCopy {
    File(Vec<u8>),
    /// A symbolic link pointing at this target.
    Link(PathBuf),
    /// A folder holding one file, as `sbx cp` copies a folder whole (P2).
    Folder,
    /// Nothing at the path ([`CopyOut::Missing`]).
    Missing,
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

    /// A backend whose `stop` fails.
    pub fn failing_stop() -> Self {
        Self {
            fail_stop: true,
            ..Self::default()
        }
    }

    /// A backend whose `template_save` fails.
    pub fn failing_template_save() -> Self {
        Self {
            fail_template_save: true,
            ..Self::default()
        }
    }

    /// A backend whose `template_rm` records the call and then fails.
    pub fn failing_template_rm(self) -> Self {
        Self {
            fail_template_rm: true,
            ..self
        }
    }

    /// A backend whose `template_rm` of `tag` records the call and then fails; other tags are
    /// removed.
    pub fn failing_template_rm_for(self, tag: &str) -> Self {
        Self {
            fail_template_rm_for: Some(tag.into()),
            ..self
        }
    }

    /// A backend whose `template_ls` fails.
    pub fn failing_template_ls(self) -> Self {
        Self {
            fail_template_ls: true,
            ..self
        }
    }

    /// A backend whose kit validation passes with this warning.
    pub fn with_kit_warning(self, warning: &str) -> Self {
        Self {
            kit_warning: Some(warning.to_owned()),
            ..self
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
            secrets: Mutex::new(names.iter().map(|n| n.to_string()).collect()),
            ..Self::default()
        }
    }

    /// Makes `secret_services` return these names, on top of another constructor.
    pub fn and_secrets(self, names: &[&str]) -> Self {
        Self {
            secrets: Mutex::new(names.iter().map(|n| n.to_string()).collect()),
            ..self
        }
    }

    /// Changes what `secret_services` returns from now on, e.g. to model a secret removed while
    /// a `--restart` confirmation is pending (issue #207): a test's `Confirm::confirm` can call
    /// this to change the answer the *next* `secret_services` call gives, after the ones a
    /// restart's before-the-prompt checks already made.
    pub fn set_secrets(&self, names: &[&str]) {
        *self.secrets.lock().unwrap() = names.iter().map(|n| n.to_string()).collect();
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

    /// Makes `list` fail with `sbx`'s own words, as the real backend passes them on.
    pub fn failing_list_with(self, message: &str) -> Self {
        Self {
            fail_list: true,
            list_failure: Some(message.to_owned()),
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

    /// Makes `create` of every sandbox whose name contains `needle` record the call and then
    /// fail, for a sandbox whose full name (a run id) isn't known in advance.
    pub fn with_failing_create_matching(mut self, needle: &str) -> Self {
        self.fail_create_matching.push(needle.to_owned());
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

    /// A handle to flip from a hook or a `Confirm`, so `secret_services` starts failing partway
    /// through a test instead of from the start, modeling `sbx` becoming unreachable mid-run or
    /// mid-list (issue #201 review M-2). Store the returned clone and call `.store(true, ..)` on
    /// it when the test's scripted moment arrives.
    pub fn secret_services_failure(&self) -> Arc<std::sync::atomic::AtomicBool> {
        self.fail_secret_services.clone()
    }

    /// A backend whose `template_ls` starts with these.
    pub fn with_templates(self, templates: Vec<TemplateImage>) -> Self {
        *self.templates.lock().unwrap() = templates;
        self
    }

    /// A backend whose `network_rules` returns these.
    pub fn with_network_rules(self, network_rules: Vec<NetworkRule>) -> Self {
        Self {
            network_rules,
            ..self
        }
    }

    /// Makes `network_rules` record the call and then fail.
    pub fn failing_network_rules(self) -> Self {
        Self {
            fail_network_rules: true,
            ..self
        }
    }

    /// Makes the next `remove` calls fail with these errors, one each, in order, and the ones
    /// after them succeed.
    pub fn with_remove_errors(self, errors: &[&str]) -> Self {
        *self.remove_errors.lock().unwrap() = errors.iter().map(|e| (*e).to_owned()).collect();
        self
    }

    /// Makes `remove` record the call and then fail, like [`FakeBackend::failing_remove`].
    pub fn and_failing_remove(self) -> Self {
        Self {
            fail_remove: true,
            ..self
        }
    }

    /// A backend whose `custom_secrets` starts with these.
    pub fn with_custom_secrets(self, secrets: Vec<CustomSecret>) -> Self {
        *self.custom_secrets.lock().unwrap() = secrets;
        self
    }

    /// Makes `custom_secrets` fail.
    pub fn failing_custom_secrets(self) -> Self {
        Self {
            fail_custom_secrets: true,
            ..self
        }
    }

    /// Makes `set_custom_secret` record the call and then fail.
    pub fn failing_set_custom_secret(self) -> Self {
        Self {
            fail_set_custom_secret: true,
            ..self
        }
    }

    /// Makes `remove_custom_secret` record the call and then fail.
    pub fn failing_remove_custom_secret(self) -> Self {
        Self {
            fail_remove_custom_secret: true,
            ..self
        }
    }

    /// Every `set_custom_secret` call, in order.
    pub fn custom_secret_sets(&self) -> Vec<CustomSecretSpec> {
        self.custom_secret_sets.lock().unwrap().clone()
    }

    /// The custom secrets that exist now: those it started with, plus each set, minus each
    /// removed.
    pub fn current_custom_secrets(&self) -> Vec<CustomSecret> {
        self.custom_secrets.lock().unwrap().clone()
    }

    /// Every location `located` was asked for, in order.
    pub fn locations(&self) -> Vec<Location> {
        self.locations.lock().unwrap().clone()
    }

    /// Makes `copy_out` of the in-sandbox `path` produce `copy`, in any sandbox.
    pub fn with_copy_out(mut self, path: &str, copy: FakeCopy) -> Self {
        self.copy_outs.insert(path.to_owned(), copy);
        self
    }

    /// Lets `root` stand in for every sandbox's filesystem in `copy_in` and `copy_out`.
    pub fn with_sandbox_root(self, root: &Path) -> Self {
        Self {
            sandbox_root: Some(root.to_path_buf()),
            ..self
        }
    }

    /// `<root>/<path>` for an in-sandbox absolute path.
    fn in_root(&self, path: &str) -> Option<PathBuf> {
        let root = self.sandbox_root.as_ref()?;
        Some(root.join(path.trim_start_matches('/')))
    }

    fn record(&self, call: &str, name: &str) {
        self.log.lock().unwrap().push(format!("{call} {name}"));
    }
}

/// Writes `copy` at `host`.
fn produce(copy: &FakeCopy, host: &Path) -> Result<CopyOut> {
    match copy {
        FakeCopy::File(bytes) => std::fs::write(host, bytes)?,
        FakeCopy::Link(target) => symlink(target, host)?,
        FakeCopy::Folder => {
            std::fs::create_dir(host)?;
            std::fs::write(host.join("inside.txt"), "a file in a folder\n")?;
        }
        FakeCopy::Missing => return Ok(CopyOut::Missing),
    }
    Ok(CopyOut::Copied)
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

/// Copies `from` to `to` as it is: a link as a link, a folder with everything in it.
fn copy_as_is(from: &Path, to: &Path) -> Result<()> {
    let kind = std::fs::symlink_metadata(from)?.file_type();
    if kind.is_symlink() {
        symlink(&std::fs::read_link(from)?, to)?;
    } else if kind.is_dir() {
        std::fs::create_dir(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_as_is(&entry.path(), &to.join(entry.file_name()))?;
        }
    } else {
        std::fs::copy(from, to)?;
    }
    Ok(())
}

impl SandboxBackend for FakeBackend {
    fn located(&self, location: Location) -> &dyn SandboxBackend {
        self.locations.lock().unwrap().push(location);
        self
    }

    fn copy_in(&self, sandbox: &str, host: &Path, path: &str) -> Result<()> {
        self.record("cp-in", &format!("{sandbox} {path}"));
        if let Some(target) = self.in_root(path) {
            if !target.parent().is_some_and(Path::is_dir) {
                bail!("fake cp: the folder of {path} does not exist in {sandbox}");
            }
            std::fs::copy(host, &target)?;
        }
        Ok(())
    }

    fn copy_out(&self, sandbox: &str, path: &str, host: &Path) -> Result<CopyOut> {
        self.record("cp-out", &format!("{sandbox} {path}"));
        if let Some(copy) = self.copy_outs.get(path) {
            return produce(copy, host);
        }
        match self.in_root(path) {
            Some(source) if std::fs::symlink_metadata(&source).is_ok() => {
                copy_as_is(&source, host)?;
                Ok(CopyOut::Copied)
            }
            _ => Ok(CopyOut::Missing),
        }
    }

    fn create(&self, spec: &CreateSpec) -> Result<()> {
        if let Some(hook) = &self.create_hook {
            (hook.0)(spec);
        }
        self.creates.lock().unwrap().push(spec.clone());
        self.record("create", &spec.name);
        if self.fail_create
            || self.fail_create_for.contains(&spec.name)
            || self
                .fail_create_matching
                .iter()
                .any(|n| spec.name.contains(n.as_str()))
        {
            bail!("fake create failure");
        }
        Ok(())
    }

    fn list(&self) -> Result<Vec<SandboxInfo>> {
        if self.no_sbx {
            bail!("fake: sbx not found");
        }
        if self.fail_list {
            match &self.list_failure {
                Some(message) => bail!("{message}"),
                None => bail!("fake: daemon not running"),
            }
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
        if self.fail_stop {
            bail!("fake stop failure");
        }
        Ok(())
    }

    fn remove(&self, name: &str) -> Result<()> {
        self.removes.lock().unwrap().push(name.to_owned());
        self.record("rm", name);
        if self.fail_remove {
            bail!("fake remove failure");
        }
        if let Some(error) = self.remove_errors.lock().unwrap().pop_front() {
            bail!("{error}");
        }
        Ok(())
    }

    fn attach(&self, name: &str) -> Result<()> {
        self.record("attach", name);
        Ok(())
    }

    fn secret_services(&self) -> Result<Vec<String>> {
        *self.secret_service_calls.lock().unwrap() += 1;
        if self
            .fail_secret_services
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            bail!("fake: sbx not reachable");
        }
        Ok(self.secrets.lock().unwrap().clone())
    }

    fn validate_kit(&self, dir: &Path) -> Result<KitValidation> {
        self.record("validate", &dir.display().to_string());
        Ok(KitValidation {
            valid: self.invalid_kit.is_none(),
            error: self.invalid_kit.clone(),
            warnings: self
                .kit_warning
                .iter()
                .map(|w| serde_json::Value::String(w.clone()))
                .collect(),
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

    fn template_save(&self, sandbox: &str, tag: &str) -> Result<()> {
        self.record("template-save", &format!("{sandbox} {tag}"));
        if self.fail_template_save {
            bail!("fake save failure");
        }
        let (name, version) = tag.split_once(':').unwrap_or((tag, "latest"));
        let mut templates = self.templates.lock().unwrap();
        // `sbx` replaces an existing tag without a question (S11, question 8).
        templates.retain(|t| t.reference() != tag);
        templates.push(TemplateImage {
            id: "fake".into(),
            repository: format!("docker.io/library/{name}"),
            tag: version.into(),
            flavor: "fake".into(),
            created_at: "2026-10-16T00:00:00Z".into(),
            size: 1_234_567,
        });
        Ok(())
    }

    fn template_ls(&self) -> Result<Vec<TemplateImage>> {
        if self.fail_template_ls {
            bail!("fake template_ls failure");
        }
        Ok(self.templates.lock().unwrap().clone())
    }

    fn network_rules(&self, sandbox: &str) -> Result<Vec<NetworkRule>> {
        self.record("network_rules", sandbox);
        if self.fail_network_rules {
            bail!("fake network_rules failure");
        }
        Ok(self.network_rules.clone())
    }

    fn custom_secrets(&self) -> Result<Vec<CustomSecret>> {
        if self.fail_custom_secrets {
            bail!("fake: cannot list custom secrets");
        }
        Ok(self.custom_secrets.lock().unwrap().clone())
    }

    fn set_custom_secret(&self, spec: &CustomSecretSpec) -> Result<()> {
        self.custom_secret_sets.lock().unwrap().push(spec.clone());
        self.record(
            "secret-set",
            &format!("{} {}", spec.sandbox, spec.placeholder),
        );
        if self.fail_set_custom_secret {
            bail!("fake set-custom failure");
        }
        self.custom_secrets.lock().unwrap().push(CustomSecret {
            scope: spec.sandbox.clone(),
            targets: vec![spec.host.clone()],
            env: spec.env.clone(),
            placeholder: spec.placeholder.clone(),
        });
        Ok(())
    }

    fn remove_custom_secret(&self, placeholder: &str, sandbox: &str) -> Result<()> {
        self.record("secret-rm", &format!("{sandbox} {placeholder}"));
        if self.fail_remove_custom_secret {
            bail!("fake secret rm failure");
        }
        // One that is already gone is removed all the same, as `SbxBackend` answers.
        self.custom_secrets
            .lock()
            .unwrap()
            .retain(|s| !(s.placeholder == placeholder && s.scope == sandbox));
        Ok(())
    }

    fn template_rm(&self, tag: &str) -> Result<()> {
        super::check_template_tag(tag)?;
        self.record("template-rm", tag);
        if self.fail_template_rm || self.fail_template_rm_for.as_deref() == Some(tag) {
            bail!("fake template rm failure");
        }
        self.templates
            .lock()
            .unwrap()
            .retain(|t| t.reference() != tag);
        Ok(())
    }
}
