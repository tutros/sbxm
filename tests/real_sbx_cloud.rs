//! Milestone 4 slice 9 (`sdlc/milestone-4.md`, work-order item 9; decisions 260, 267): the walk
//! against the real Docker Cloud Sandboxes. It costs money, so every test is `#[ignore]`d **and**
//! skipped (saying why) unless `SBXM_REAL_CLOUD=1` and `SBXM_REAL_BASE_DIR` are set. Run with:
//! `SBXM_REAL_CLOUD=1 SBXM_REAL_BASE_DIR=<existing dir, not on C: (decision 56)> cargo test --test
//! real_sbx_cloud -- --ignored`
//! Needs `sbx` 0.46.0 or later logged in to the cloud, with the `anthropic` OAuth login stored
//! there (`sbx secret set anthropic --oauth --cloud`). Every sandbox is on the small shape (2 CPUs,
//! 4 GiB).
//!
//! Decision 267's captures: [`cloud_walk_against_real_sbx`] writes any removal answer that says
//! "temporarily unavailable" to `src/backend/fixtures/sbx-cloud-rm-unavailable.txt`, which replaces
//! slice 3's S14-worded text (the `unavailable()` helpers in `tests/task_lifetime.rs`,
//! `tests/task_cloud.rs` and the `UNAVAILABLE` constants of `tests/task_cloud_rm.rs` and
//! `tests/task_cloud_finish.rs`); [`capture_not_logged_in`], run by hand after `sbx logout` with
//! `SBXM_REAL_CLOUD_LOGGED_OUT=1`, writes `src/backend/fixtures/sbx-cloud-not-logged-in.txt`, which
//! replaces slice 2's made-up `src/backend/fixtures/sbx-cloud-ls-not-logged-in-made-up.txt`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use sbxm::backend::{
    CopyOut, CreateSpec, CustomSecret, CustomSecretSpec, ExecOutput, ExecSpec, KitValidation,
    Location, NetworkRule, SandboxBackend, SandboxInfo, SbxBackend, ScopedSecret, TemplateImage,
};
use tempfile::TempDir;

/// Why a test is skipped, or `None` when it may run: the cloud costs money, so the variable that
/// says so is needed besides the base dir.
fn skip_reason() -> Option<&'static str> {
    if std::env::var("SBXM_REAL_CLOUD").as_deref() != Ok("1") {
        return Some(
            "skipped: set SBXM_REAL_CLOUD=1 to run it against the real Docker Cloud Sandboxes \
             (it costs money)",
        );
    }
    if std::env::var_os("SBXM_REAL_BASE_DIR").is_none() {
        return Some("skipped: set SBXM_REAL_BASE_DIR to an existing dir, not on C: (decision 56)");
    }
    None
}

fn logged_out_capture() -> bool {
    std::env::var("SBXM_REAL_CLOUD_LOGGED_OUT").as_deref() == Ok("1")
}

/// Where a captured fixture goes, in the crate's own `src/backend/fixtures/`.
fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("backend")
        .join("fixtures")
        .join(name)
}

/// The anthropic hosts a strict `claude` cloud sandbox keeps (P9 (a), `src/template.rs`), plus the
/// profile's `[template].allow` below.
const RUNTIME_HOSTS: [&str; 4] = [
    "api.anthropic.com",
    "claude.ai",
    "console.anthropic.com",
    ALLOWED_HOST,
];

/// The one host the walk's strict profile allows; S14 saw it answer 200 from the cloud.
const ALLOWED_HOST: &str = "example.com";

/// Hosts the walk's sandbox must not reach: one on no list (S14 saw it refused) and one of the
/// built-in list (S17) that the strict removal denies.
const DENIED_HOSTS: [&str; 2] = ["www.wikipedia.org", "github.com"];

/// The environment variable the fake scoped secret's placeholder is in.
const SECRET_ENV: &str = "SBXM_IT_FAKE";

/// Which of S14's three refusal shapes a `curl -w '%{http_code} %{http_connect}'` shows, if any:
/// a DNS failure (exit 6), connection refused (exit 7), or the proxy's 403.
fn refusal_shape(exit: Option<i32>, stdout: &str) -> Option<&'static str> {
    let mut codes = stdout.split_whitespace();
    let (code, connect) = (codes.next(), codes.next());
    if code == Some("403") || connect == Some("403") {
        Some("proxy 403")
    } else if exit == Some(6) {
        Some("DNS failure")
    } else if exit == Some(7) {
        Some("connection refused")
    } else {
        None
    }
}

/// `sbx --cloud <args>`, never prompting.
fn sbx_cloud(args: &[&str]) -> std::process::Output {
    Command::new("sbx")
        .arg("--cloud")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("`sbx` runs")
}

/// `curl` from inside `sandbox` to `url`: its exit code and `<http_code> <http_connect>`.
fn curl(sandbox: &str, url: &str) -> (Option<i32>, String) {
    let out = sbx_cloud(&[
        "exec",
        sandbox,
        "curl",
        "-sS",
        "-o",
        "/dev/null",
        "--max-time",
        "30",
        "-w",
        "%{http_code} %{http_connect}",
        url,
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    println!(
        "{url}: exit {:?}, {stdout}, {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    (out.status.code(), stdout)
}

/// What the walk saw inside the worker sandbox while it ran, read before its first `exec` (after
/// sbxm's strict check, which comes before any `exec`, decision 255).
#[derive(Debug, Default)]
struct Live {
    /// `sbx --cloud ls`'s entry for it: expiry, CPUs, memory.
    expires_at: Option<String>,
    shape: (Option<u32>, Option<u64>),
    /// `sbx --cloud policy ls <name>`'s allowed hosts, or why they couldn't be read; `None`
    /// before it is read.
    allowed_hosts: Option<Result<Vec<String>, String>>,
    /// `<http_code> <http_connect>` from [`ALLOWED_HOST`].
    allowed_answer: String,
    /// Each of [`DENIED_HOSTS`] with the refusal shape it showed (`None`: not refused).
    denied: Vec<(String, Option<&'static str>, String)>,
    /// The fake secret's placeholder in [`SECRET_ENV`], the files holding its value, and the
    /// count of process environments holding it.
    placeholder: String,
    files_with_value: String,
    envs_with_value: String,
}

/// The cloud instance of [`SbxBackend`], watched: it records each create (name and TTL), sets the
/// fake scoped secret before the worker's create (as sbxm does for a scoped secret, slice 4, which
/// P8 (a) gives no production value yet), runs the [`Live`] checks before the worker sandbox's
/// first `exec`, and saves a removal error that says "temporarily unavailable" (decision 267).
struct Watched {
    worker: String,
    secret: ScopedSecret,
    /// Every sandbox name the walk may have made, for the guard: the expected ones up front, plus
    /// any other `create` seen.
    names: Mutex<Vec<String>>,
    creates: Mutex<Vec<(String, Option<Duration>)>>,
    live: Mutex<Option<Live>>,
    unavailable_saved: Mutex<bool>,
}

const CLOUD: &SbxBackend = &SbxBackend::CLOUD;

impl Watched {
    fn note(&self, name: &str) {
        let mut names = self.names.lock().unwrap();
        if !names.iter().any(|n| n == name) {
            names.push(name.to_owned());
        }
    }

    fn observe(&self, sandbox: &str) -> Live {
        let mut live = Live::default();
        match CLOUD.list_detailed() {
            Ok(list) => {
                if let Some(entry) = list.into_iter().find(|s| s.name == sandbox) {
                    live.expires_at = entry.expires_at;
                    live.shape = (entry.cpus, entry.memory_mib);
                }
            }
            Err(e) => println!("sbx --cloud ls: {e:#}"),
        }
        live.allowed_hosts = Some(
            CLOUD
                .cloud_allowed_hosts(sandbox)
                .map_err(|e| format!("{e:#}")),
        );
        live.allowed_answer = curl(sandbox, &format!("https://{ALLOWED_HOST}")).1;
        for host in DENIED_HOSTS {
            let (exit, codes) = curl(sandbox, &format!("https://{host}"));
            live.denied
                .push((host.to_owned(), refusal_shape(exit, &codes), codes));
        }
        // The value goes in as two halves, so no command line in the sandbox holds it whole but
        // grep's own (in /proc, which the file search leaves out). Run as the sandbox's default
        // user: files it can't read are skipped.
        let half = self.secret.value.len() / 2;
        let script = format!(
            "v=\"$1$2\"; \
             echo \"placeholder=${{{SECRET_ENV}:-}}\"; \
             echo \"files=$(timeout 600 grep -rlsF --exclude-dir=proc --exclude-dir=sys \
             --exclude-dir=dev -- \"$v\" / 2>/dev/null | head -5 | tr '\\n' ' ')\"; \
             echo \"envs=$(for f in /proc/[0-9]*/environ; do tr '\\0' '\\n' < \"$f\" 2>/dev/null; \
             done | grep -cF -- \"$v\")\""
        );
        let out = sbx_cloud(&[
            "exec",
            sandbox,
            "sh",
            "-c",
            &script,
            "sh",
            &self.secret.value[..half],
            &self.secret.value[half..],
        ]);
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if let Some(v) = line.strip_prefix("placeholder=") {
                live.placeholder = v.to_owned();
            } else if let Some(v) = line.strip_prefix("files=") {
                live.files_with_value = v.trim().to_owned();
            } else if let Some(v) = line.strip_prefix("envs=") {
                live.envs_with_value = v.trim().to_owned();
            }
        }
        live
    }
}

impl SandboxBackend for Watched {
    fn located(&self, location: Location) -> &dyn SandboxBackend {
        match location {
            Location::Cloud => self,
            Location::Local => &SbxBackend::LOCAL,
        }
    }
    fn create(&self, spec: &CreateSpec) -> Result<()> {
        self.note(&spec.name);
        self.creates
            .lock()
            .unwrap()
            .push((spec.name.clone(), spec.ttl));
        if spec.name == self.worker {
            CLOUD.set_scoped_secret(&spec.name, &self.secret)?;
        }
        CLOUD.create(spec)
    }
    fn copy_in(&self, sandbox: &str, host: &Path, path: &str) -> Result<()> {
        CLOUD.copy_in(sandbox, host, path)
    }
    fn copy_out(&self, sandbox: &str, path: &str, host: &Path) -> Result<CopyOut> {
        CLOUD.copy_out(sandbox, path, host)
    }
    fn list(&self) -> Result<Vec<SandboxInfo>> {
        CLOUD.list()
    }
    fn list_detailed(&self) -> Result<Vec<sbxm::backend::SandboxDetail>> {
        CLOUD.list_detailed()
    }
    fn stop(&self, name: &str) -> Result<()> {
        CLOUD.stop(name)
    }
    fn remove(&self, name: &str) -> Result<()> {
        let result = CLOUD.remove(name);
        if let Err(e) = &result {
            let error = format!("{e:#}");
            println!("removal of {name}: {error}");
            if error.contains("temporarily unavailable") {
                let mut saved = self.unavailable_saved.lock().unwrap();
                if !*saved {
                    // `SbxBackend::remove` puts sbx's stderr after "(<exit status>): ".
                    let text = error.split_once("): ").map_or(error.as_str(), |(_, t)| t);
                    // Never a panic here: the guard's `Drop` removes through this too.
                    let path = fixture_path("sbx-cloud-rm-unavailable.txt");
                    match std::fs::write(&path, format!("{text}\n")) {
                        Ok(()) => println!("captured {}", path.display()),
                        Err(e) => println!("cannot write {}: {e}", path.display()),
                    }
                    *saved = true;
                }
            }
        }
        result
    }
    fn attach(&self, name: &str) -> Result<()> {
        CLOUD.attach(name)
    }
    fn validate_kit(&self, dir: &Path) -> Result<KitValidation> {
        CLOUD.validate_kit(dir)
    }
    fn secret_services(&self) -> Result<Vec<String>> {
        CLOUD.secret_services()
    }
    fn version(&self) -> Result<String> {
        CLOUD.version()
    }
    fn exec(&self, sandbox: &str, spec: &ExecSpec) -> Result<ExecOutput> {
        if sandbox == self.worker {
            let mut live = self.live.lock().unwrap();
            if live.is_none() {
                *live = Some(self.observe(sandbox));
            }
        }
        CLOUD.exec(sandbox, spec)
    }
    fn skills(&self) -> Result<serde_json::Value> {
        CLOUD.skills()
    }
    fn template_save(&self, sandbox: &str, tag: &str) -> Result<()> {
        CLOUD.template_save(sandbox, tag)
    }
    fn template_ls(&self) -> Result<Vec<TemplateImage>> {
        CLOUD.template_ls()
    }
    fn template_rm(&self, tag: &str) -> Result<()> {
        CLOUD.template_rm(tag)
    }
    fn network_rules(&self, sandbox: &str) -> Result<Vec<NetworkRule>> {
        CLOUD.network_rules(sandbox)
    }
    fn cloud_allowed_hosts(&self, sandbox: &str) -> Result<Vec<String>> {
        CLOUD.cloud_allowed_hosts(sandbox)
    }
    fn cloud_deny_host(&self, sandbox: &str, host: &str) -> Result<()> {
        CLOUD.cloud_deny_host(sandbox, host)
    }
    fn custom_secrets(&self) -> Result<Vec<CustomSecret>> {
        CLOUD.custom_secrets()
    }
    fn set_custom_secret(&self, spec: &CustomSecretSpec) -> Result<()> {
        CLOUD.set_custom_secret(spec)
    }
    fn remove_custom_secret(&self, placeholder: &str, sandbox: &str) -> Result<()> {
        CLOUD.remove_custom_secret(placeholder, sandbox)
    }
    fn set_scoped_secret(&self, sandbox: &str, secret: &ScopedSecret) -> Result<()> {
        self.note(sandbox);
        CLOUD.set_scoped_secret(sandbox, secret)
    }
    fn scoped_secrets(&self, sandbox: &str) -> Result<Vec<String>> {
        CLOUD.scoped_secrets(sandbox)
    }
    fn remove_secret(&self, service: &str, sandbox: &str) -> Result<()> {
        CLOUD.remove_secret(service, sandbox)
    }
}

/// What of the walk's is still in the cloud: sandboxes `sbx --cloud ls` lists, and secrets `sbx
/// --cloud secret ls` scopes to one of them. Empty when it is clean; an entry when the cloud
/// couldn't be read.
fn left_in_cloud(names: &[String]) -> Vec<String> {
    let mut left = Vec::new();
    match CLOUD.list() {
        Ok(list) => left.extend(
            list.into_iter()
                .filter(|s| names.contains(&s.name))
                .map(|s| format!("sandbox {}", s.name)),
        ),
        Err(e) => left.push(format!("`sbx --cloud ls` unreadable: {e:#}")),
    }
    for name in names {
        match CLOUD.scoped_secrets(name) {
            Ok(secrets) => {
                left.extend(secrets.into_iter().map(|s| format!("secret {s} of {name}")))
            }
            Err(e) => left.push(format!("`sbx --cloud secret ls` unreadable: {e:#}")),
        }
    }
    left
}

/// Removes every sandbox and scoped secret the walk made, and its folders, on every path; then
/// checks that `sbx --cloud ls` and `sbx --cloud secret ls` hold nothing of the walk's.
struct Guard {
    watched: Arc<Watched>,
    dirs: Vec<PathBuf>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        let names = self.watched.names.lock().unwrap().clone();
        for name in &names {
            // Retried as sbxm retries (P6), through `Watched` so a retryable answer is captured.
            let removal = sbxm::task::lifetime::remove_sandbox(
                self.watched.as_ref(),
                name,
                &std::thread::sleep,
            );
            println!("cleanup: {name}: {removal:?}");
            if let Ok(secrets) = CLOUD.scoped_secrets(name) {
                for service in secrets {
                    if let Err(e) = CLOUD.remove_secret(&service, name) {
                        println!("cleanup: secret {service} of {name}: {e:#}");
                    }
                }
            }
        }
        for dir in &self.dirs {
            let _ = std::fs::remove_dir_all(dir);
        }
        let left = left_in_cloud(&names);
        if left.is_empty() {
            println!("cleanup: nothing of the walk is left in the cloud");
        } else if std::thread::panicking() {
            eprintln!("cleanup: still in the cloud, remove by hand: {left:?}");
        } else {
            panic!("cleanup: still in the cloud, remove by hand: {left:?}");
        }
    }
}

fn real_git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// `sbx --cloud policy ls [<sandbox>] --json`'s allowed and denied hosts, or `None` when it fails.
fn policy(sandbox: Option<&str>) -> Option<(serde_json::Value, serde_json::Value)> {
    let mut args = vec!["policy", "ls"];
    args.extend(sandbox);
    args.push("--json");
    let out = sbx_cloud(&args);
    println!(
        "sbx --cloud {}: {} {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stdout).trim(),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    if !out.status.success() {
        return None;
    }
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    Some((json["allowNetworks"].clone(), json["denyNetworks"].clone()))
}

const WORKER_PROMPT: &str = "Do exactly this and nothing else:\n\
     1. Create a file hello.txt containing the word hi.\n\
     2. Run: git add hello.txt && git commit -m \"Add hello.txt\"\n\
     3. Write .sbxm-task/result.md saying: hello.txt added.\n\
     4. Stop.\n";

const REVIEWER_PROMPT: &str = "Do exactly this and nothing else: write the file {{review_path}} \
     with these lines, and stop:\n\nMust-fix findings: 0\n\nRisk: low\n\nNothing found.\n";

/// The walk (decision 260): one tiny spec task through `task run --cloud` to `ready`, on a
/// strict profile, with a fake scoped secret on the worker sandbox; while the worker sandbox runs,
/// its TTL, shape, policy, egress and secret are checked; afterwards the bundle out (the worker's
/// commit in `repo.git`) and the removals. Spends a few cents of cloud time and Anthropic credit.
#[test]
#[ignore = "needs SBXM_REAL_CLOUD=1, SBXM_REAL_BASE_DIR and a cloud login; costs money"]
fn cloud_walk_against_real_sbx() {
    use sbxm::commands::task_run::{self, SpecOptions};
    use sbxm::harness::Harness;
    use sbxm::task::record::{self, Stage, SystemProbe};
    use sbxm::task::repo;

    if let Some(why) = skip_reason() {
        println!("{why}");
        return;
    }
    if logged_out_capture() {
        println!(
            "skipped: SBXM_REAL_CLOUD_LOGGED_OUT=1 is for capture_not_logged_in; unset it once \
             you are logged in again"
        );
        return;
    }
    let tmp = TempDir::new().unwrap();
    let base_dir = PathBuf::from(std::env::var_os("SBXM_REAL_BASE_DIR").unwrap())
        .join(format!("sbxm-it-{}-cloud", std::process::id()));
    std::fs::create_dir_all(&base_dir).unwrap();

    // The spec task, its id and its sandboxes' names.
    let spec = tmp.path().join("idea.md");
    std::fs::write(&spec, "Add a hello.txt file that contains the word hi.\n").unwrap();
    let id = record::spec_id(&spec).unwrap().0;
    let worker = format!("sbxm-task-{id}-claude");
    let reviewer = format!("sbxm-task-{id}-review-claude");
    let value = format!(
        "sbxm-it-fake-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let watched = Arc::new(Watched {
        worker: worker.clone(),
        secret: ScopedSecret {
            host: ALLOWED_HOST.into(),
            env: SECRET_ENV.into(),
            value: value.clone(),
        },
        names: Mutex::new(vec![worker.clone(), reviewer.clone()]),
        creates: Mutex::new(Vec::new()),
        live: Mutex::new(None),
        unavailable_saved: Mutex::new(false),
    });
    let _guard = Guard {
        watched: Arc::clone(&watched),
        dirs: vec![base_dir.clone()],
    };
    let account_policy = policy(None);

    // The config: a strict profile allowing one host, on the small shape.
    let config_dir = tmp.path().join("config");
    let profile_dir = config_dir.join("profiles").join("default");
    std::fs::create_dir_all(&profile_dir).unwrap();
    let base = toml::Value::String(base_dir.to_str().unwrap().to_owned());
    let profiles = toml::Value::String(config_dir.join("profiles").to_str().unwrap().to_owned());
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "base_dir = {base}\nprofiles_dir = {profiles}\ndefault_profile = \"default\"\n\n\
             [resources]\ncpus = 2\nmemory = \"4g\"\n"
        ),
    )
    .unwrap();
    std::fs::write(
        profile_dir.join("profile.toml"),
        format!(
            "description = \"real cloud test\"\n\n[network]\nallow = [\"{ALLOWED_HOST}\"]\n\n\
             [template]\nstrict = true\nallow = [\"{ALLOWED_HOST}\"]\n"
        ),
    )
    .unwrap();

    // A scratch repo standing in for GitHub, and the target repo's config.
    let origin = tmp.path().join("origin.git");
    let seed = tmp.path().join("seed");
    std::fs::create_dir_all(&origin).unwrap();
    std::fs::create_dir_all(&seed).unwrap();
    real_git(&origin, &["init", "--bare", "-b", "main"]);
    real_git(&seed, &["init", "-b", "main"]);
    std::fs::write(seed.join("README.md"), "hello\n").unwrap();
    real_git(&seed, &["add", "-A"]);
    real_git(&seed, &["commit", "-m", "first"]);
    real_git(&seed, &["push", origin.to_str().unwrap(), "main"]);
    let target = tmp.path().join("target");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(
        target.join("sbxm-task.toml"),
        "[sandbox]\nprofile = \"default\"\ncpus = 2\nmemory = \"4g\"\n\n[gates]\nsandbox = []\n\n\
         [prompts]\nworker = \"worker.md\"\nreviewer = \"reviewer.md\"\n",
    )
    .unwrap();
    std::fs::write(target.join("worker.md"), WORKER_PROMPT).unwrap();
    std::fs::write(target.join("reviewer.md"), REVIEWER_PROMPT).unwrap();

    let started = std::time::Instant::now();
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = task_run::run_spec(
        &config_dir,
        &SpecOptions {
            repo_root: target.clone(),
            spec,
            worker_harness: Some(Harness::Claude),
            worker_model: Some("claude-haiku-4-5-20251001".into()),
            reviewer_harness: Some(Harness::Claude),
            reviewer_model: Some("claude-haiku-4-5-20251001".into()),
            time_limit: Some("10m".into()),
            reviewer_time_limit: Some("10m".into()),
            profile: None,
            base: Some("main".into()),
            repo: Some("o/r".into()),
            clone_source: Some(origin.to_str().unwrap().to_owned()),
            identity: Some(repo::Identity {
                name: "Dev".into(),
                email: "dev@example.com".into(),
            }),
            disk: Default::default(),
            location: Some(Location::Cloud),
        },
        None,
        watched.as_ref(),
        &sbxm::github::fake::FakeGitHub::default(),
        &SystemProbe,
        &sbxm::task::gates::ShellHostRunner,
        &mut out,
        &mut warn,
    );
    println!(
        "took {:?}\n{}\n{}",
        started.elapsed(),
        String::from_utf8_lossy(&out),
        String::from_utf8_lossy(&warn)
    );
    result.unwrap();

    // `ready`, recorded in the cloud, every sandbox with its TTL and removed.
    let meta = record::task_dir(&base_dir, &id);
    let task = record::read(&meta.join("task.json")).unwrap();
    assert_eq!(task.stage, Stage::Ready, "{task:?}");
    assert_eq!(task.location, Location::Cloud);
    assert!(!task.sandboxes.is_empty(), "{task:?}");
    for life in &task.sandboxes {
        assert!(life.ttl_s > 0, "{life:?}");
        assert!(life.removed_at.is_some(), "{life:?}");
    }
    let run_log = std::fs::read_to_string(meta.join("run.log")).unwrap();
    assert!(
        run_log.contains(&format!("cloud sandbox {worker} created")),
        "{run_log}"
    );
    assert!(
        run_log.contains(&format!("strict cloud sandbox {worker}: sbxm denied")),
        "{run_log}"
    );

    // The TTL in the create, and in `ls` while it ran.
    let creates = watched.creates.lock().unwrap().clone();
    assert!(
        creates.iter().any(|(n, _)| *n == worker) && creates.iter().any(|(n, _)| *n == reviewer),
        "{creates:?}"
    );
    assert!(creates.iter().all(|(_, ttl)| ttl.is_some()), "{creates:?}");
    let live = watched
        .live
        .lock()
        .unwrap()
        .take()
        .expect("the worker sandbox ran an exec");
    println!("{live:?}");
    assert!(
        live.expires_at.as_deref().is_some_and(|e| !e.is_empty()),
        "{live:?}"
    );
    assert_eq!(live.shape, (Some(2), Some(4096)), "the small shape");

    // A strict sandbox's list after the denies holds only the runtime list.
    let hosts = live
        .allowed_hosts
        .as_ref()
        .expect("policy ls read")
        .as_ref()
        .expect("policy ls readable");
    for entry in hosts {
        let host = entry.split(':').next().unwrap_or(entry);
        assert!(RUNTIME_HOSTS.contains(&host), "{host} allowed: {hosts:?}");
    }

    // Egress: the allowed host answers, the denied ones fail in any of S14's three shapes.
    assert!(
        live.allowed_answer.starts_with("200"),
        "{ALLOWED_HOST}: {}",
        live.allowed_answer
    );
    for (host, shape, codes) in &live.denied {
        assert!(shape.is_some(), "{host} was not refused: {codes}");
    }

    // The fake scoped secret: only its placeholder is in the sandbox.
    assert!(
        !live.placeholder.is_empty() && live.placeholder != value,
        "{SECRET_ENV}={}",
        live.placeholder
    );
    assert_eq!(live.files_with_value, "", "files holding the value");
    assert_eq!(live.envs_with_value, "0", "environments holding the value");

    // The bundle in and out: the worker's commit is in `repo.git`.
    let repo_git = meta.join("repo.git");
    assert!(repo::commits_ahead(&repo_git, "main", &task.branch).unwrap() >= 1);
    assert!(real_git(&repo_git, &["show", &format!("{}:hello.txt", task.branch)]).contains("hi"));

    // Removal: no sandbox of the task in `sbx --cloud ls`, none of its rules in `policy ls`, the
    // account's policy as it was. A gone sandbox's `policy ls` may fail or show the account's.
    let names = watched.names.lock().unwrap().clone();
    let listed: Vec<String> = CLOUD.list().unwrap().into_iter().map(|s| s.name).collect();
    for name in &names {
        assert!(!listed.contains(name), "{name} still listed: {listed:?}");
        if let Some(rules) = policy(Some(name)) {
            assert_eq!(Some(rules), account_policy, "rules left for {name}");
        }
    }
    assert_eq!(policy(None), account_policy, "the account's policy changed");
}

/// Decision 267's first capture, run by hand after `sbx logout` with
/// `SBXM_REAL_CLOUD_LOGGED_OUT=1`: what `sbx --cloud ls --json`, the call sbxm's cloud preflight
/// makes, says when not logged in. Replaces slice 2's made-up
/// `sbx-cloud-ls-not-logged-in-made-up.txt`.
#[test]
#[ignore = "needs SBXM_REAL_CLOUD=1, SBXM_REAL_BASE_DIR, SBXM_REAL_CLOUD_LOGGED_OUT=1, sbx logout"]
fn capture_not_logged_in() {
    if let Some(why) = skip_reason() {
        println!("{why}");
        return;
    }
    if !logged_out_capture() {
        println!(
            "skipped: run `sbx logout`, then set SBXM_REAL_CLOUD_LOGGED_OUT=1 to capture the \
             not-logged-in message"
        );
        return;
    }
    let out = sbx_cloud(&["ls", "--json"]);
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
    println!("exit {:?}: {stderr}", out.status.code());
    assert!(
        !stderr.is_empty(),
        "no message on stderr; is sbx logged out?"
    );
    std::fs::write(
        fixture_path("sbx-cloud-not-logged-in.txt"),
        format!("{stderr}\n"),
    )
    .unwrap();
}
