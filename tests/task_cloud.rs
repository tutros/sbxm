//! Milestone 4 slice 1 (`sdlc/milestone-4.md`, decisions 252-267): `task start --spec FILE
//! --cloud`, non-strict, through `FakeBackend`.

mod common;

use common::task_fixture::fixture_with;
use sbxm::backend::Location;
use sbxm::task::pipeline::choose_location;

const LOCAL_TOML: &str = "[sandbox]\nprofile = \"default\"\nlocation = \"local\"\n\n[gates]\nsandbox = [\"cargo test\"]\n";
const CLOUD_TOML: &str = "[sandbox]\nprofile = \"default\"\nlocation = \"cloud\"\n\n[gates]\nsandbox = [\"cargo test\"]\n";
const NO_LOCATION_TOML: &str =
    "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n";

/// P14 (decision 262(k)): the flag, else the config.
#[test]
fn the_flag_wins_over_the_config_and_the_config_decides_without_one() {
    let local = fixture_with(LOCAL_TOML).config;
    let cloud = fixture_with(CLOUD_TOML).config;
    let absent = fixture_with(NO_LOCATION_TOML).config;

    assert_eq!(
        choose_location(Some(Location::Cloud), &local),
        Location::Cloud
    );
    assert_eq!(
        choose_location(Some(Location::Local), &cloud),
        Location::Local
    );
    assert_eq!(choose_location(None, &cloud), Location::Cloud);
    assert_eq!(choose_location(None, &local), Location::Local);
    assert_eq!(choose_location(None, &absent), Location::Local);
}

fn identity() -> sbxm::task::repo::Identity {
    sbxm::task::repo::Identity {
        name: "Dev".into(),
        email: "dev@example.com".into(),
    }
}

/// The cloud sandbox's home, as `printenv HOME` reports it in the fake.
const HOME: &str = "/home/agent";

/// A fake cloud: `<root>` stands in for the sandbox's filesystem (copy in and out land there),
/// `printenv HOME` answers [`HOME`], and the sandbox's own `sh`, `git` and `mkdir` commands really
/// run on the host with their in-sandbox paths moved under `<root>`. The worker's headless run
/// commits `a.txt` in its clone and writes `result.md`; every other command (the gates) passes.
fn cloud_backend(root: &std::path::Path, id: &str) -> sbxm::backend::FakeBackend {
    use common::task_fixture::{CLAUDE_DONE, ok};

    cloud_backend_with(root, id, ok(CLAUDE_DONE))
}

/// [`cloud_backend`] whose worker's headless run answers `worker`.
fn cloud_backend_with(
    root: &std::path::Path,
    id: &str,
    worker: sbxm::backend::ExecOutput,
) -> sbxm::backend::FakeBackend {
    use common::task_fixture::{is_headless, ok};

    std::fs::create_dir_all(root.join(HOME.trim_start_matches('/'))).unwrap();
    let inside = root.to_string_lossy().into_owned();
    let map = move |arg: &str| arg.replace(HOME, &format!("{inside}{HOME}"));
    let worker_map = map.clone();
    let clone = format!("{HOME}/work/{id}");
    cloud_ready()
        .with_sandbox_root(root)
        .with_exec_output_matching("printenv", ok(&format!("{HOME}\n")))
        .with_exec_output_matching("claude", worker)
        .with_exec_hook(move |_sandbox, spec| {
            if is_headless(spec) {
                let clone = std::path::PathBuf::from(worker_map(&clone));
                std::fs::write(clone.join("a.txt"), "from the cloud\n").unwrap();
                common::git(&clone, &["add", "-A"]);
                common::git(&clone, &["commit", "-q", "-m", "a.txt"]);
                std::fs::write(clone.join(".sbxm-task").join("result.md"), "done\n").unwrap();
            }
        })
        .with_exec_responder(move |_sandbox, spec| {
            if !matches!(spec.argv[0].as_str(), "sh" | "git" | "mkdir") {
                return None;
            }
            let mut command = std::process::Command::new(&spec.argv[0]);
            command.args(spec.argv[1..].iter().map(|a| map(a)));
            if let Some(dir) = &spec.workdir {
                command.current_dir(map(&dir.to_string_lossy()));
            }
            let out = command.output().unwrap();
            Some(sbxm::backend::ExecOutput {
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                exit_code: out.status.code(),
            })
        })
}

fn spec_options(
    f: &common::task_fixture::Fixture,
    spec: std::path::PathBuf,
    location: Option<Location>,
) -> sbxm::commands::task_start::SpecOptions {
    sbxm::commands::task_start::SpecOptions {
        repo_root: f.env.tmp.path().join("target-repo"),
        spec,
        worker_harness: None,
        worker_model: None,
        time_limit: None,
        reviewer_time_limit: None,
        profile: None,
        base: Some("main".into()),
        repo: Some("o/r".into()),
        clone_source: Some(common::task_fixture::source(f)),
        identity: Some(identity()),
        disk: Default::default(),
        location,
    }
}

/// `task start --spec FILE` with `--cloud` (or `None`, leaving it to the config).
fn start_spec(
    f: &common::task_fixture::Fixture,
    backend: &sbxm::backend::FakeBackend,
    location: Option<Location>,
) -> (anyhow::Result<()>, String) {
    let spec = common::task_fixture::spec_file(f, "Build a thing.\n");
    let mut out = Vec::new();
    let result = sbxm::commands::task_start::run_spec(
        &f.env.config_dir(),
        &spec_options(f, spec, location),
        backend,
        &sbxm::github::fake::FakeGitHub::default(),
        &common::task_fixture::Probe,
        &mut out,
        &mut Vec::new(),
    );
    (result, String::from_utf8(out).unwrap())
}

fn spec_id(f: &common::task_fixture::Fixture) -> String {
    let path = f.env.tmp.path().join("idea.md");
    std::fs::write(&path, "Build a thing.\n").unwrap();
    sbxm::task::record::spec_id(&path).unwrap().0
}

/// The backend's state-changing calls in order, each `exec` named by what it ran.
fn described_log(backend: &sbxm::backend::FakeBackend) -> Vec<String> {
    let mut execs = backend.execs().into_iter();
    backend
        .log()
        .into_iter()
        .filter(|entry| !entry.starts_with("validate "))
        .map(|entry| {
            if !entry.starts_with("exec ") {
                return entry;
            }
            let (_, spec) = execs.next().unwrap();
            let argv = spec.argv.join(" ");
            let what = if argv.contains("printenv HOME") {
                "printenv HOME"
            } else if argv.contains("git clone") {
                "clone"
            } else if spec.argv[0] == "mkdir" {
                "mkdir"
            } else if argv.contains("claude") {
                "worker"
            } else if argv.contains("bundle create") {
                "bundle"
            } else if argv.contains("status --porcelain") {
                "status"
            } else if argv.contains("cargo test") {
                "gate"
            } else {
                "other"
            };
            format!("exec {what}")
        })
        .collect()
}

/// Done when (slice 1): the backend log in order, `repo.git` holding the worker's commit, and
/// `task.json` saying `cloud` with the clone path.
#[test]
fn a_cloud_spec_start_clones_runs_gates_and_collects_through_copies() {
    let f = fixture_with(LOCAL_TOML);
    let id = spec_id(&f);
    let root = f.env.tmp.path().join("cloud-fs");
    let backend = cloud_backend(&root, &id);
    let sandbox = format!("sbxm-task-{id}-claude");
    let clone = format!("{HOME}/work/{id}");

    let (result, out) = start_spec(&f, &backend, Some(Location::Cloud));

    result.unwrap();
    assert!(
        out.contains("1 commit") && out.contains("gates: passed"),
        "{out}"
    );
    assert_eq!(backend.locations(), [Location::Cloud]);
    assert_eq!(
        described_log(&backend),
        [
            format!("create {sandbox}"),
            "exec printenv HOME".to_owned(),
            format!("cp-in {sandbox} {HOME}/.sbxm-task-{id}.bundle"),
            "exec clone".to_owned(),
            "exec mkdir".to_owned(),
            format!("cp-in {sandbox} {clone}/.sbxm-task/source.md"),
            format!("cp-in {sandbox} {clone}/.sbxm-task/prompt.md"),
            "exec worker".to_owned(),
            "exec bundle".to_owned(),
            format!("cp-out {sandbox} {clone}/.sbxm-task/branch.bundle"),
            "exec status".to_owned(),
            format!("cp-out {sandbox} {clone}/.sbxm-task/result.md"),
            "exec gate".to_owned(),
        ]
    );

    // Created with the profile's two kits, in order, and no workspace.
    let create = &backend.creates()[0];
    assert_eq!(create.workspace, None);
    assert_eq!(create.template, None);
    // With the worker TTL (slice 3, P5 (d)).
    assert_eq!(
        create.ttl,
        Some(sbxm::task::lifetime::worker_ttl(&f.config).unwrap())
    );
    let kits: Vec<_> = create
        .kits
        .iter()
        .map(|k| k.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(kits, ["common", "harness-claude"]);

    // The worker, the gates and the collection all ran in the recorded clone.
    for (_, spec) in backend.execs() {
        let argv = spec.argv.join(" ");
        if argv.contains("claude") || argv.contains("cargo test") {
            assert_eq!(
                spec.workdir.as_deref(),
                Some(std::path::Path::new(&clone)),
                "{argv}"
            );
        }
    }

    let meta = sbxm::task::record::task_dir(&f.env.base_dir(), &id);
    let task = sbxm::task::record::read(&meta.join("task.json")).unwrap();
    assert_eq!(task.location, Location::Cloud);
    assert_eq!(task.worker.as_ref().unwrap().workspace, clone);
    // One lifetime entry, still open: the start hands over to the review, which continues the
    // automatic run in the same sandbox (slice 3, P5 (d), P7 (a)); one line at its create.
    let ttl = sbxm::task::lifetime::worker_ttl(&f.config).unwrap();
    assert_eq!(task.sandboxes.len(), 1, "{:?}", task.sandboxes);
    let entry = &task.sandboxes[0];
    assert_eq!(entry.name, sandbox);
    assert_eq!(entry.role, sbxm::task::record::SandboxRole::Worker);
    assert_eq!(entry.location, Location::Cloud);
    assert_eq!(entry.ttl_s, ttl.as_secs());
    assert_eq!(entry.removed_at, None);
    assert!(
        sbxm::run::results::parse_timestamp(&entry.created_at).is_some(),
        "{}",
        entry.created_at
    );
    let created = format!(
        "cloud sandbox {sandbox} created (worker, TTL {})",
        sbxm::backend::ttl_arg(ttl)
    );
    assert_eq!(out.matches(&created).count(), 1, "{out}");
    assert!(!out.contains("removed"), "{out}");
    assert!(backend.removes().is_empty());
    let log = common::git(
        &meta.join("repo.git"),
        &["log", "--format=%s", &format!("refs/heads/{id}")],
    );
    assert!(log.lines().next() == Some("a.txt"), "{log}");
    assert_eq!(
        std::fs::read_to_string(meta.join("result.md")).unwrap(),
        "done\n"
    );
    // No host clone, and every staging folder gone once read.
    assert!(!f.env.base_dir().join("tasks").join(&id).exists());
    let staged = std::fs::read_dir(meta.join("staging")).map_or(0, |d| d.count());
    assert_eq!(staged, 0);
}

/// `[sandbox] location = "cloud"` alone takes the cloud path too.
#[test]
fn the_config_alone_starts_a_cloud_task() {
    let f = fixture_with(CLOUD_TOML);
    let id = spec_id(&f);
    let backend = cloud_backend(&f.env.tmp.path().join("cloud-fs"), &id);

    let (result, _) = start_spec(&f, &backend, None);

    result.unwrap();
    assert_eq!(backend.locations(), [Location::Cloud]);
    assert_eq!(backend.creates()[0].workspace, None);
}

/// The biggest `result.md` brought to the host (the pipeline's `RESULT_CAP`).
const RESULT_CAP: usize = 1024 * 1024;

fn result_path(id: &str) -> String {
    format!("{HOME}/work/{id}/.sbxm-task/result.md")
}

fn bundle_path(id: &str) -> String {
    format!("{HOME}/work/{id}/.sbxm-task/branch.bundle")
}

fn task_record(f: &common::task_fixture::Fixture, id: &str) -> sbxm::task::record::Record {
    let meta = sbxm::task::record::task_dir(&f.env.base_dir(), id);
    sbxm::task::record::read(&meta.join("task.json")).unwrap()
}

/// Starts a cloud task whose copy out of `path` produces `copy`; returns the result, the record
/// and the copied-in `result.md` if any.
fn start_with_copy(
    path: impl Fn(&str) -> String,
    copy: sbxm::backend::FakeCopy,
) -> (
    anyhow::Result<()>,
    sbxm::task::record::Record,
    Option<Vec<u8>>,
) {
    let f = fixture_with(LOCAL_TOML);
    let id = spec_id(&f);
    let backend =
        cloud_backend(&f.env.tmp.path().join("cloud-fs"), &id).with_copy_out(&path(&id), copy);
    let (result, _) = start_spec(&f, &backend, Some(Location::Cloud));
    let meta = sbxm::task::record::task_dir(&f.env.base_dir(), &id);
    let staged = std::fs::read_dir(meta.join("staging")).map_or(0, |d| d.count());
    assert_eq!(staged, 0, "a staging folder was left behind");
    let result_md = std::fs::read(meta.join("result.md")).ok();
    (result, task_record(&f, &id), result_md)
}

fn notes(record: &sbxm::task::record::Record) -> String {
    record.notes.join("\n")
}

/// Decision 244's boundary: a staged `result.md` at its cap is read, one byte over is not.
#[test]
fn a_staged_result_at_its_cap_is_read_and_one_byte_over_is_not() {
    use sbxm::backend::FakeCopy;

    let (result, _, copied) = start_with_copy(result_path, FakeCopy::File(vec![b'x'; RESULT_CAP]));
    result.unwrap();
    assert_eq!(copied.map(|c| c.len()), Some(RESULT_CAP));

    let (result, record, copied) =
        start_with_copy(result_path, FakeCopy::File(vec![b'x'; RESULT_CAP + 1]));
    result.unwrap();
    assert_eq!(copied, None);
    assert!(
        notes(&record).contains("larger than 1 MiB"),
        "{}",
        notes(&record)
    );
}

/// The cloud refuses a symlink but copies a folder whole (P2); whatever lands, only one plain file
/// is read.
#[test]
fn a_staged_result_that_is_a_link_or_a_folder_is_refused() {
    use sbxm::backend::FakeCopy;

    for copy in [FakeCopy::Link("/etc/passwd".into()), FakeCopy::Folder] {
        let (result, record, copied) = start_with_copy(result_path, copy.clone());
        result.unwrap();
        assert_eq!(copied, None, "{copy:?}");
        assert!(
            notes(&record).contains("result.md was refused")
                && notes(&record).contains("not a plain file"),
            "{copy:?}: {}",
            notes(&record)
        );
    }
}

#[test]
fn a_staged_bundle_that_is_a_link_or_a_folder_is_refused_and_the_worker_fails() {
    use sbxm::backend::FakeCopy;

    for copy in [FakeCopy::Link("/etc/passwd".into()), FakeCopy::Folder] {
        let (result, record, _) = start_with_copy(bundle_path, copy.clone());
        assert!(result.is_err(), "{copy:?}");
        assert_eq!(
            record.status,
            sbxm::task::record::Status::Failed,
            "{copy:?}"
        );
        assert!(
            notes(&record).contains("could not collect the worker's commits")
                && notes(&record).contains("not a plain file"),
            "{copy:?}: {}",
            notes(&record)
        );
    }
}

/// A missing bundle (the copy out finds nothing) is an error, never an empty success.
#[test]
fn a_bundle_missing_from_the_sandbox_fails_the_worker() {
    let (result, record, _) = start_with_copy(bundle_path, sbxm::backend::FakeCopy::Missing);
    assert!(result.is_err());
    assert!(notes(&record).contains("cannot find"), "{}", notes(&record));
}

/// The bundle's cap applies to the staged copy as it did to the mounted one: at the cap it is
/// fetched, one byte over it is refused.
#[test]
fn a_staged_bundle_at_its_cap_is_fetched_and_one_byte_over_is_refused() {
    use sbxm::backend::FakeCopy;
    use sbxm::task::pipeline::copy_out_staged;
    use sbxm::task::repo;

    let f = fixture_with(LOCAL_TOML);
    let meta = f.env.tmp.path().join("meta");
    std::fs::create_dir_all(&meta).unwrap();
    let repo_git = meta.join("repo.git");
    repo::clone_bare(&common::task_fixture::source(&f), &repo_git).unwrap();
    repo::create_branch(&repo_git, "work", "main").unwrap();
    // A real bundle with one commit on `work`, made the way the sandbox makes it.
    let scratch = f.env.tmp.path().join("scratch");
    common::git(
        f.env.tmp.path(),
        &[
            "clone",
            "-q",
            repo_git.to_str().unwrap(),
            scratch.to_str().unwrap(),
        ],
    );
    common::git(&scratch, &["checkout", "-q", "work"]);
    std::fs::write(scratch.join("b.txt"), "b\n").unwrap();
    common::git(&scratch, &["add", "-A"]);
    common::git(&scratch, &["commit", "-q", "-m", "b"]);
    common::git(
        &scratch,
        &[
            "bundle",
            "create",
            "-q",
            "../w.bundle",
            "work",
            "^origin/main",
        ],
    );
    let bytes = std::fs::read(f.env.tmp.path().join("w.bundle")).unwrap();
    let len = bytes.len() as u64;
    let clone = "/home/agent/work/t";
    let backend = sbxm::backend::FakeBackend::default().with_copy_out(
        &format!("{clone}/.sbxm-task/branch.bundle"),
        FakeCopy::File(bytes),
    );

    let staged = copy_out_staged(&backend, &meta, "sb", clone, "branch.bundle").unwrap();
    let over = repo::fetch_bundle(&repo_git, staged.dir(), "work", len - 1).unwrap_err();
    assert!(format!("{over:#}").contains("too large"), "{over:#}");
    repo::fetch_bundle(&repo_git, staged.dir(), "work", len).unwrap();
    assert_eq!(
        backend.log(),
        [format!("cp-out sb {clone}/.sbxm-task/branch.bundle")]
    );
    let dir = staged.dir().to_path_buf();
    drop(staged);
    assert!(!dir.exists(), "the staging folder goes with its Staged");
}

/// Decision 262(g): a non-strict `[template]` profile is built from its kits, with one line.
#[test]
fn a_template_profile_is_built_from_its_kits_with_one_line_in_the_cloud() {
    let f = fixture_with(LOCAL_TOML);
    f.env
        .write_profile("default", "[template]\nallow = [\"crates.io\"]\n");
    let backend = cloud_backend(&f.env.tmp.path().join("cloud-fs"), &spec_id(&f));
    let spec = common::task_fixture::spec_file(&f, "Build a thing.\n");
    let mut warn = Vec::new();

    sbxm::commands::task_start::run_spec(
        &f.env.config_dir(),
        &spec_options(&f, spec, Some(Location::Cloud)),
        &backend,
        &sbxm::github::fake::FakeGitHub::default(),
        &common::task_fixture::Probe,
        &mut Vec::new(),
        &mut warn,
    )
    .unwrap();

    let warn = String::from_utf8(warn).unwrap();
    assert_eq!(
        warn.matches("cloud templates aren't used").count(),
        1,
        "{warn}"
    );
    assert_eq!(backend.creates()[0].template, None);
}

/// A sandbox whose HOME isn't an absolute path gets no clone: the start fails and is undone.
#[test]
fn a_home_that_is_not_an_absolute_path_fails_the_start_and_undoes_it() {
    let f = fixture_with(LOCAL_TOML);
    let id = spec_id(&f);
    let backend =
        cloud_ready().with_exec_output_matching("printenv", common::task_fixture::ok("work\n"));

    let (result, _) = start_spec(&f, &backend, Some(Location::Cloud));

    let message = format!("{:#}", result.unwrap_err());
    assert!(message.contains("HOME"), "{message}");
    let sandbox = format!("sbxm-task-{id}-claude");
    assert_eq!(backend.removes(), [sandbox]);
    assert!(!sbxm::task::record::task_dir(&f.env.base_dir(), &id).exists());
    assert!(!backend.log().iter().any(|e| e.starts_with("cp-in")));
}

/// Self-review: a failed cloud start removes only what it made. It never reserves a host clone, so
/// a folder that appears at `<base>/tasks/<id>` while the sandbox is being set up isn't its own to
/// delete.
#[test]
fn a_failed_cloud_start_leaves_a_host_folder_it_did_not_make() {
    let f = fixture_with(LOCAL_TOML);
    let id = spec_id(&f);
    let folder = f.env.base_dir().join("tasks").join(&id);
    let made = folder.clone();
    let backend = cloud_ready()
        .with_exec_output_matching("printenv", common::task_fixture::ok("work\n"))
        .with_create_hook(move |_| {
            std::fs::create_dir_all(&made).unwrap();
            std::fs::write(made.join("keep.txt"), "not sbxm's\n").unwrap();
        });

    let (result, _) = start_spec(&f, &backend, Some(Location::Cloud));

    assert!(result.is_err());
    assert!(folder.join("keep.txt").is_file());
    assert!(!sbxm::task::record::task_dir(&f.env.base_dir(), &id).exists());
}

/// Review M-1: the sandbox name is the task's, so one may already exist; a failed start removes it
/// only if this start's own `create` made it. A failure before the create (here the clone of the
/// source), or the create itself failing, removes nothing. (A failure after a successful create
/// removes that sandbox once: `a_home_that_is_not_an_absolute_path_fails_the_start_and_undoes_it`.)
#[test]
fn a_cloud_start_that_fails_before_or_at_its_create_removes_no_sandbox() {
    let f = fixture_with(LOCAL_TOML);
    let id = spec_id(&f);
    let sandbox = format!("sbxm-task-{id}-claude");

    let failing = cloud_ready().with_failing_create_for(&sandbox);
    let (result, _) = start_spec(&f, &failing, Some(Location::Cloud));
    let message = format!("{:#}", result.unwrap_err());
    assert!(message.contains(&sandbox), "{message}");
    assert_eq!(failing.removes(), Vec::<String>::new());
    assert!(!sbxm::task::record::task_dir(&f.env.base_dir(), &id).exists());

    let backend = cloud_ready();
    let spec = common::task_fixture::spec_file(&f, "Build a thing.\n");
    let mut options = spec_options(&f, spec, Some(Location::Cloud));
    let missing = f.env.tmp.path().join("no-such-repo");
    options.clone_source = Some(missing.to_string_lossy().into_owned());
    let result = sbxm::commands::task_start::run_spec(
        &f.env.config_dir(),
        &options,
        &backend,
        &sbxm::github::fake::FakeGitHub::default(),
        &common::task_fixture::Probe,
        &mut Vec::new(),
        &mut Vec::new(),
    );
    assert!(result.is_err());
    assert!(backend.creates().is_empty(), "{:?}", backend.log());
    assert_eq!(backend.removes(), Vec::<String>::new());
    assert!(!sbxm::task::record::task_dir(&f.env.base_dir(), &id).exists());
}

// Slice 2: the cloud preflight (`sdlc/milestone-4.md`, "Preflight refusals", P8, P11, P12;
// decisions 256, 261, 262, 267). Every refusal comes before any write, delete or changing backend
// call, before the task folder exists, and ends with "or run it here with `--local`".

/// The cloud minimum (P12).
const CLOUD_MIN: &str = "0.46.0";

/// A fake cloud that passes every preflight check: `sbx` at the cloud minimum, the cloud
/// answering `ls` with no sandboxes, and the worker's `anthropic` login stored globally.
fn cloud_ready() -> sbxm::backend::FakeBackend {
    common::task_fixture::backend().with_version(CLOUD_MIN)
}

/// Asserts a preflight refusal: the message's ending, no changing backend call, no task folder.
/// Returns the message for the caller's own checks.
fn assert_preflight_refusal(
    result: anyhow::Result<()>,
    f: &common::task_fixture::Fixture,
    backend: &sbxm::backend::FakeBackend,
) -> String {
    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.ends_with("or run it here with `--local`"),
        "{message}"
    );
    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert!(!f.env.base_dir().join(".sbxm").join("tasks").exists());
    message
}

/// P12, decision 244's boundary: one under the cloud minimum is refused, naming the version found
/// and the minimum; at it and one over are accepted.
#[test]
fn an_sbx_under_the_cloud_minimum_is_refused_and_at_or_over_it_is_accepted() {
    let f = fixture_with(LOCAL_TOML);
    let backend = cloud_ready().with_version("0.45.9");

    let (result, _) = start_spec(&f, &backend, Some(Location::Cloud));

    let message = assert_preflight_refusal(result, &f, &backend);
    assert!(
        message.contains("sbx 0.45.9") && message.contains(CLOUD_MIN),
        "{message}"
    );

    for version in [CLOUD_MIN, "0.46.1"] {
        let f = fixture_with(LOCAL_TOML);
        let id = spec_id(&f);
        let backend = cloud_backend(&f.env.tmp.path().join("cloud-fs"), &id).with_version(version);
        let (result, _) = start_spec(&f, &backend, Some(Location::Cloud));
        result.unwrap();
        assert_eq!(backend.creates().len(), 1, "{version}");
    }
}

/// Decision 267: the real not-logged-in message was never captured, so this one is made up, and
/// sbxm matches no wording of it.
const NOT_LOGGED_IN: &str =
    include_str!("../src/backend/fixtures/sbx-cloud-ls-not-logged-in-made-up.txt");

/// The "not logged in" row (decision 267): the probe `sbx --cloud ls --json` failing for any
/// reason refuses with `sbx`'s own words and `sbx login`, as an `SbxUnavailable`, so a `task run`
/// list stops as a whole (decision 193(a)). The empty list `sbx-cloud-ls-empty.json` stands for
/// passes: `cloud_ready` answers it, and every successful start above went through it.
#[test]
fn a_cloud_that_does_not_answer_ls_is_refused_naming_sbx_login() {
    let f = fixture_with(LOCAL_TOML);
    let backend = cloud_ready().failing_list_with(NOT_LOGGED_IN.trim());

    let (result, _) = start_spec(&f, &backend, Some(Location::Cloud));

    let error = result.unwrap_err();
    assert!(
        error.is::<sbxm::task::pipeline::SbxUnavailable>(),
        "{error:#}"
    );
    let message = assert_preflight_refusal(Err(error), &f, &backend);
    assert!(
        message.contains(NOT_LOGGED_IN.trim())
            && message.contains("if you're not logged in, run `sbx login`"),
        "{message}"
    );
}

/// The "no cloud value source" row (P8 (a), decision 262(f)): a `secrets.services` entry that
/// isn't one of the two provider OAuth logins, `google` included, is refused naming the secret
/// and the profile, before the cloud's logins are looked at (`anthropic` is stored here).
#[test]
fn a_profile_secret_with_no_cloud_value_source_is_refused_naming_it_and_the_profile() {
    for secret in ["github", "google"] {
        let f = fixture_with(LOCAL_TOML);
        f.env.write_profile(
            "default",
            &format!("[secrets]\nservices = [\"{secret}\"]\n"),
        );
        let backend = cloud_ready();

        let (result, _) = start_spec(&f, &backend, Some(Location::Cloud));

        let message = assert_preflight_refusal(result, &f, &backend);
        assert!(
            message.contains(&format!("'{secret}'")) && message.contains("profile 'default'"),
            "{message}"
        );
    }
}

/// A worker whose own provider login isn't one the cloud stores (Antigravity's `google`) is
/// refused the same way, naming the harness, instead of being sent to a `sbx secret set google
/// --oauth --cloud` that doesn't exist.
#[test]
fn a_worker_harness_whose_provider_has_no_cloud_login_is_refused() {
    let f = fixture_with(LOCAL_TOML);
    let backend = cloud_ready().and_secrets(&["google"]);
    let spec = common::task_fixture::spec_file(&f, "Build a thing.\n");
    let mut options = spec_options(&f, spec, Some(Location::Cloud));
    options.worker_harness = Some(sbxm::harness::Harness::Antigravity);

    let result = sbxm::commands::task_start::run_spec(
        &f.env.config_dir(),
        &options,
        &backend,
        &sbxm::github::fake::FakeGitHub::default(),
        &common::task_fixture::Probe,
        &mut Vec::new(),
        &mut Vec::new(),
    );

    let message = assert_preflight_refusal(result, &f, &backend);
    assert!(
        message.contains("antigravity") && message.contains("'google'"),
        "{message}"
    );
}

/// The "provider OAuth login missing" row (P8 (d), decision 256): the harness's own login absent
/// from the cloud is a `MissingSecret` (a `task run` list stops on it, decision 193(a)) naming the
/// exact `sbx secret set <provider> --oauth --cloud`. The scope rule, a global entry counts and a
/// sandbox-scoped one doesn't, is the parser's: `the_cloud_secret_ls_keeps_the_global_service_logins`
/// and `a_login_only_at_a_sandbox_scope_is_not_listed` in `backend::sbx`.
#[test]
fn a_provider_login_missing_in_the_cloud_is_refused_naming_the_oauth_command() {
    let f = fixture_with(LOCAL_TOML);
    let backend = cloud_ready().and_secrets(&[]);

    let (result, _) = start_spec(&f, &backend, Some(Location::Cloud));

    let error = result.unwrap_err();
    assert!(
        error.is::<sbxm::task::pipeline::MissingSecret>(),
        "{error:#}"
    );
    let message = assert_preflight_refusal(Err(error), &f, &backend);
    assert!(
        message.contains("'anthropic'")
            && message.contains("`sbx secret set anthropic --oauth --cloud`"),
        "{message}"
    );
}

/// The opposite-provider case the plan names: a `codex` worker whose profile names `anthropic`,
/// with `anthropic` missing in the cloud, is refused naming `sbx secret set anthropic --oauth
/// --cloud`; with both logins stored it starts.
#[test]
fn a_codex_task_whose_profile_names_anthropic_needs_that_login_in_the_cloud_too() {
    let start = |backend: &sbxm::backend::FakeBackend| {
        let f = fixture_with(LOCAL_TOML);
        f.env
            .write_profile("default", "[secrets]\nservices = [\"anthropic\"]\n");
        let spec = common::task_fixture::spec_file(&f, "Build a thing.\n");
        let mut options = spec_options(&f, spec, Some(Location::Cloud));
        options.worker_harness = Some(sbxm::harness::Harness::Codex);
        let result = sbxm::commands::task_start::run_spec(
            &f.env.config_dir(),
            &options,
            backend,
            &sbxm::github::fake::FakeGitHub::default(),
            &common::task_fixture::Probe,
            &mut Vec::new(),
            &mut Vec::new(),
        );
        (f, result)
    };

    let backend = cloud_ready().and_secrets(&["openai"]);
    let (f, result) = start(&backend);
    let message = assert_preflight_refusal(result, &f, &backend);
    assert!(
        message.contains("'anthropic'")
            && message.contains("profile 'default'")
            && message.contains("`sbx secret set anthropic --oauth --cloud`"),
        "{message}"
    );

    let backend = cloud_ready().and_secrets(&["openai", "anthropic"]);
    let (_, result) = start(&backend);
    // Past the preflight: the start reached the cloud (and failed there, in this bare fake).
    assert!(result.is_err());
    assert_eq!(backend.creates().len(), 1, "{:?}", backend.log());
}

/// P11, decision 262(h): a cloud create doesn't use the drive holding `sbx`'s local sandbox
/// state, so a cloud start goes past the free-disk check; a local one is still refused by it.
#[test]
fn too_little_free_disk_refuses_a_local_start_but_not_a_cloud_one() {
    let start = |location, ignore| {
        let f = fixture_with(LOCAL_TOML);
        let id = spec_id(&f);
        let backend = cloud_backend(&f.env.tmp.path().join("cloud-fs"), &id);
        let spec = common::task_fixture::spec_file(&f, "Build a thing.\n");
        let mut options = spec_options(&f, spec, Some(location));
        options.disk = sbxm::disk::Guard {
            ignore,
            ..common::low_disk(&f.env)
        };
        let result = sbxm::commands::task_start::run_spec(
            &f.env.config_dir(),
            &options,
            &backend,
            &sbxm::github::fake::FakeGitHub::default(),
            &common::task_fixture::Probe,
            &mut Vec::new(),
            &mut Vec::new(),
        );
        (f, backend, result)
    };

    let (f, backend, result) = start(Location::Local, false);
    let message = format!("{:#}", result.unwrap_err());
    assert!(message.starts_with(common::LOW_DISK), "{message}");
    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert!(!f.env.base_dir().join(".sbxm").join("tasks").exists());

    // `--ignore-disk` is still accepted for a cloud start and changes nothing.
    for ignore in [false, true] {
        let (_, backend, result) = start(Location::Cloud, ignore);
        result.unwrap();
        assert_eq!(backend.creates().len(), 1, "ignore {ignore}");
    }
}

// Slice 3: the worker sandbox is removed when the task stops (P5 (d), decision 263), with the
// removal retry (P6 (a)), its lifetime entry stamped (P7 (a)) and one line for `run.log` (265).

/// What a start printed and the waits its removal asked for.
struct Started {
    result: anyhow::Result<()>,
    out: String,
    warn: String,
    waits: Vec<std::time::Duration>,
}

/// `task start --spec FILE --cloud` with the removal's sleep recorded instead of slept.
fn start_cloud(f: &common::task_fixture::Fixture, backend: &sbxm::backend::FakeBackend) -> Started {
    start_cloud_with_secrets(f, backend, &[])
}

/// [`start_cloud`] with the task's scoped secrets (slice 4): empty in production, so only a test
/// fills the list, with a fake value.
fn start_cloud_with_secrets(
    f: &common::task_fixture::Fixture,
    backend: &sbxm::backend::FakeBackend,
    scoped_secrets: &[sbxm::backend::ScopedSecret],
) -> Started {
    let spec = common::task_fixture::spec_file(f, "Build a thing.\n");
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let waits = std::sync::Mutex::new(Vec::new());
    let result = sbxm::commands::task_start::run_spec_with_seams(
        &f.env.config_dir(),
        &spec_options(f, spec, Some(Location::Cloud)),
        backend,
        &sbxm::github::fake::FakeGitHub::default(),
        &common::task_fixture::Probe,
        &mut out,
        &mut warn,
        &|d| waits.lock().unwrap().push(d),
        scoped_secrets,
    );
    Started {
        result,
        out: String::from_utf8(out).unwrap(),
        warn: String::from_utf8(warn).unwrap(),
        waits: waits.into_inner().unwrap(),
    }
}

/// The worker's headless run, failing as the harness reports it.
fn failed_worker() -> sbxm::backend::ExecOutput {
    sbxm::backend::ExecOutput {
        stdout: String::new(),
        stderr: "boom".into(),
        exit_code: Some(1),
    }
}

fn unavailable(sandbox: &str) -> String {
    format!(
        "`sbx rm` failed for sandbox {sandbox} (exit status: 1): error: sandbox {sandbox} is \
         temporarily unavailable; retry the delete"
    )
}

fn secs(list: &[u64]) -> Vec<std::time::Duration> {
    list.iter()
        .map(|s| std::time::Duration::from_secs(*s))
        .collect()
}

/// Asserts the task's one worker entry was stamped removed, and the line for it printed.
fn assert_removed(f: &common::task_fixture::Fixture, id: &str, started: &Started, how: &str) {
    let sandbox = format!("sbxm-task-{id}-claude");
    let task = task_record(f, id);
    assert_eq!(task.sandboxes.len(), 1, "{:?}", task.sandboxes);
    let entry = &task.sandboxes[0];
    assert_eq!(entry.name, sandbox);
    let removed = entry.removed_at.as_deref().expect("removed_at is set");
    let (created, removed) = (
        sbxm::run::results::parse_timestamp(&entry.created_at).unwrap(),
        sbxm::run::results::parse_timestamp(removed).unwrap(),
    );
    assert!(removed >= created);
    let line = format!("cloud sandbox {sandbox} {how}");
    assert_eq!(started.out.matches(&line).count(), 1, "{}", started.out);
}

#[test]
fn a_failed_worker_stops_the_task_and_its_cloud_sandbox_is_removed() {
    let f = fixture_with(LOCAL_TOML);
    let id = spec_id(&f);
    let backend = cloud_backend_with(&f.env.tmp.path().join("cloud-fs"), &id, failed_worker());

    let started = start_cloud(&f, &backend);

    assert!(started.result.is_err());
    let sandbox = format!("sbxm-task-{id}-claude");
    assert_eq!(backend.removes(), [sandbox]);
    assert_eq!(backend.locations(), [Location::Cloud]);
    assert_removed(&f, &id, &started, "removed");
    assert!(started.waits.is_empty());
    // The gates never ran: the task stopped at its worker.
    assert!(!described_log(&backend).contains(&"exec gate".to_owned()));
}

/// A timed-out cloud worker stops the task too (slice 3, behavior 2): its partial commits are
/// kept, no gates run, no review is named, and its sandbox is removed once.
#[test]
fn a_timed_out_worker_keeps_its_commits_stops_the_task_and_its_cloud_sandbox_is_removed() {
    let f = fixture_with(LOCAL_TOML);
    let id = spec_id(&f);
    let timed_out = sbxm::backend::ExecOutput {
        stdout: String::new(),
        stderr: "timeout: sending signal TERM to command 'claude'\n".into(),
        exit_code: Some(124),
    };
    let backend = cloud_backend_with(&f.env.tmp.path().join("cloud-fs"), &id, timed_out);

    let started = start_cloud(&f, &backend);

    let message = format!("{:#}", started.result.as_ref().unwrap_err());
    assert!(message.contains("timed out"), "{message}");
    assert!(
        started.out.contains("worker timed out, 1 commit(s)"),
        "{}",
        started.out
    );
    assert!(
        !started.out.contains("next: sbxm task review"),
        "{}",
        started.out
    );
    assert!(!described_log(&backend).contains(&"exec gate".to_owned()));
    assert_eq!(backend.removes(), [format!("sbxm-task-{id}-claude")]);
    assert_removed(&f, &id, &started, "removed");
}

/// A worker that couldn't be run at all (`exec` itself failing) stops the task too.
#[test]
fn a_worker_that_cannot_run_stops_the_task_and_its_cloud_sandbox_is_removed() {
    let f = fixture_with(LOCAL_TOML);
    let id = spec_id(&f);
    let backend =
        cloud_backend(&f.env.tmp.path().join("cloud-fs"), &id).with_failing_exec_matching("claude");

    let started = start_cloud(&f, &backend);

    assert!(started.result.is_err());
    assert_eq!(backend.removes(), [format!("sbxm-task-{id}-claude")]);
    assert_removed(&f, &id, &started, "removed");
}

/// A gate that could not run at all stops the task (issue 186): nothing for a review to react to.
#[test]
fn a_gate_that_could_not_run_stops_the_task_and_its_cloud_sandbox_is_removed() {
    let f = fixture_with(LOCAL_TOML);
    let id = spec_id(&f);
    let backend = cloud_backend(&f.env.tmp.path().join("cloud-fs"), &id)
        .with_failing_exec_matching("cargo test");

    let started = start_cloud(&f, &backend);

    let message = format!("{:#}", started.result.as_ref().unwrap_err());
    assert!(message.contains("could not run"), "{message}");
    assert_eq!(backend.removes(), [format!("sbxm-task-{id}-claude")]);
    assert_removed(&f, &id, &started, "removed");
}

/// Gates that ran and failed hand over to the review (decision 180), which continues the
/// automatic run in the same sandbox: it is kept.
#[test]
fn gates_that_ran_and_failed_hand_over_and_keep_the_cloud_sandbox() {
    let f = fixture_with(LOCAL_TOML);
    let id = spec_id(&f);
    let backend = cloud_backend(&f.env.tmp.path().join("cloud-fs"), &id).with_exec_output_matching(
        "cargo test",
        sbxm::backend::ExecOutput {
            stdout: String::new(),
            stderr: "test failed".into(),
            exit_code: Some(101),
        },
    );

    let started = start_cloud(&f, &backend);

    let error = started.result.as_ref().unwrap_err();
    assert!(
        error.is::<sbxm::commands::task_start::GatesFailed>(),
        "{error:#}"
    );
    assert!(backend.removes().is_empty(), "{:?}", backend.log());
    let task = task_record(&f, &id);
    assert_eq!(task.sandboxes[0].removed_at, None);
    assert!(!started.out.contains("removed"), "{}", started.out);
}

#[test]
fn a_sandbox_already_gone_at_the_stop_counts_as_removed() {
    let f = fixture_with(LOCAL_TOML);
    let id = spec_id(&f);
    let sandbox = format!("sbxm-task-{id}-claude");
    let gone = format!(
        "`sbx rm` failed for sandbox {sandbox} (exit status: 1): error: sandbox '{sandbox}' not \
         found\n  try: sbx --cloud ls"
    );
    let backend = cloud_backend_with(&f.env.tmp.path().join("cloud-fs"), &id, failed_worker())
        .with_remove_errors(&[&gone]);

    let started = start_cloud(&f, &backend);

    assert!(started.result.is_err());
    assert_eq!(backend.removes(), [sandbox]);
    assert_removed(&f, &id, &started, "already gone");
}

/// The first attempt and three retries all fail: reported as a warning naming the sandbox and the
/// command to remove it, never dropped, and the entry keeps `removed_at: null`.
#[test]
fn a_removal_that_still_fails_at_the_stop_is_reported_and_the_entry_stays_open() {
    let f = fixture_with(LOCAL_TOML);
    let id = spec_id(&f);
    let sandbox = format!("sbxm-task-{id}-claude");
    let error = unavailable(&sandbox);
    let backend = cloud_backend_with(&f.env.tmp.path().join("cloud-fs"), &id, failed_worker())
        .with_remove_errors(&[&error, &error, &error, &error]);

    let started = start_cloud(&f, &backend);

    assert!(started.result.is_err());
    assert_eq!(backend.removes().len(), 4);
    assert_eq!(started.waits, secs(&[5, 10, 20]));
    let command = format!("sbx --cloud rm {sandbox} -f");
    assert!(
        started.warn.contains(&sandbox) && started.warn.contains(&command),
        "{}",
        started.warn
    );
    assert!(
        started.warn.contains("temporarily unavailable"),
        "{}",
        started.warn
    );
    let task = task_record(&f, &id);
    assert_eq!(task.sandboxes[0].removed_at, None);
}

/// A start that fails after its create removes the sandbox with the same retries; what it did
/// (the create, the removal) is printed as at any stop, and the task's folder goes as before once
/// the sandbox is gone.
#[test]
fn a_start_that_fails_after_its_create_removes_the_sandbox_with_retries() {
    let f = fixture_with(LOCAL_TOML);
    let id = spec_id(&f);
    let sandbox = format!("sbxm-task-{id}-claude");
    let error = unavailable(&sandbox);

    // Succeeds on the third retry.
    let backend = cloud_ready()
        .with_exec_output_matching("printenv", common::task_fixture::ok("work\n"))
        .with_remove_errors(&[&error, &error, &error]);
    let started = start_cloud(&f, &backend);
    let message = format!("{:#}", started.result.as_ref().unwrap_err());
    assert!(
        message.contains("HOME") && !message.contains("rm"),
        "{message}"
    );
    assert_eq!(backend.removes().len(), 4);
    assert_eq!(started.waits, secs(&[5, 10, 20]));
    assert!(!sbxm::task::record::task_dir(&f.env.base_dir(), &id).exists());
    assert_start_lines(&started, &sandbox, "removed");

    // Already gone when the start fails: counts as removed, the folder goes.
    let gone = format!(
        "`sbx rm` failed for sandbox {sandbox} (exit status: 1): error: sandbox '{sandbox}' not \
         found\n  try: sbx --cloud ls"
    );
    let backend = cloud_ready()
        .with_exec_output_matching("printenv", common::task_fixture::ok("work\n"))
        .with_remove_errors(&[&gone]);
    let started = start_cloud(&f, &backend);
    assert!(started.result.is_err());
    assert_eq!(backend.removes(), std::slice::from_ref(&sandbox));
    assert!(!sbxm::task::record::task_dir(&f.env.base_dir(), &id).exists());
    assert_start_lines(&started, &sandbox, "already gone");
}

/// Asserts a failed start's screen shows the create once and the removal once, as `run.log` tees
/// them (decision 265), and no removal warning.
fn assert_start_lines(started: &Started, sandbox: &str, how: &str) {
    let created = format!("cloud sandbox {sandbox} created (worker, TTL ");
    assert_eq!(started.out.matches(&created).count(), 1, "{}", started.out);
    let removed = format!("cloud sandbox {sandbox} {how}");
    assert_eq!(started.out.matches(&removed).count(), 1, "{}", started.out);
    assert!(
        !started.warn.contains("could not be removed"),
        "{}",
        started.warn
    );
}

/// Review M-1: a start that fails after its create, whose removal still fails after the retries,
/// keeps the task's folder: `task.json` is the only record of a cloud sandbox that may still be
/// running (its entry keeps `removed_at: null`, behavior 3), and the screen, so `run.log`, shows
/// the create and the failed removal (behavior 5). The error names the manual removal command.
#[test]
fn a_start_that_fails_after_its_create_and_cannot_remove_the_sandbox_keeps_the_record() {
    let f = fixture_with(LOCAL_TOML);
    let id = spec_id(&f);
    let sandbox = format!("sbxm-task-{id}-claude");
    let error = unavailable(&sandbox);
    let backend = cloud_ready()
        .with_exec_output_matching("printenv", common::task_fixture::ok("work\n"))
        .with_remove_errors(&[&error, &error, &error, &error]);

    let started = start_cloud(&f, &backend);

    let message = format!("{:#}", started.result.as_ref().unwrap_err());
    let command = format!("sbx --cloud rm {sandbox} -f");
    assert!(
        message.contains("HOME") && message.contains(&command) && message.contains(&id),
        "{message}"
    );
    // The only recovery named is the manual cloud removal: `task rm` doesn't follow the recorded
    // location until slice 6 (review round 3, M-1).
    assert!(!message.contains("task rm"), "{message}");
    assert_eq!(backend.removes().len(), 4);
    assert_eq!(started.waits, secs(&[5, 10, 20]));

    let task = task_record(&f, &id);
    assert_eq!(task.location, Location::Cloud);
    assert_eq!(task.sandboxes.len(), 1, "{:?}", task.sandboxes);
    assert_eq!(task.sandboxes[0].name, sandbox);
    assert_eq!(task.sandboxes[0].removed_at, None);

    let created = format!("cloud sandbox {sandbox} created (worker, TTL ");
    assert_eq!(started.out.matches(&created).count(), 1, "{}", started.out);
    assert!(!started.out.contains(" removed"), "{}", started.out);
    let failed = format!("warning: cloud sandbox {sandbox} could not be removed: ");
    assert_eq!(started.warn.matches(&failed).count(), 1, "{}", started.warn);
    assert!(
        started.warn.contains(&command) && started.warn.contains("temporarily unavailable"),
        "{}",
        started.warn
    );
}

/// A worker TTL that can't be added up is refused with the preflight, before any write or create.
#[test]
fn a_worker_ttl_too_large_is_refused_before_anything() {
    let f = fixture_with(&format!(
        "[sandbox]\nprofile = \"default\"\n[worker]\ntime_limit = \"{}s\"\n\
         [gates]\nsandbox = [\"cargo test\"]\n",
        u64::MAX
    ));
    let id = spec_id(&f);
    let backend = cloud_ready();

    let started = start_cloud(&f, &backend);

    let message = format!("{:#}", started.result.unwrap_err());
    assert!(message.contains("time_limit"), "{message}");
    assert!(backend.creates().is_empty(), "{:?}", backend.log());
    assert!(!sbxm::task::record::task_dir(&f.env.base_dir(), &id).exists());
}

// Slice 4: scoped secrets (`sdlc/milestone-4.md` P8, decisions 256, 262(f); S14 Q2; P2 point 6).
// Every scoped secret is set before the sandbox's `create` and removed after the sandbox on every
// path; the cloud's own list says what to remove; the value is written nowhere.

const FAKE_VALUE: &str = "fake-scoped-value-NOT-REAL";

/// The one scoped secret the tests use, with a fake value (no production path fills one in while
/// P8 (a) holds).
fn fake_secret() -> sbxm::backend::ScopedSecret {
    sbxm::backend::ScopedSecret {
        host: "postman-echo.com".into(),
        env: "P2_FAKE".into(),
        value: FAKE_VALUE.into(),
    }
}

/// The service name `sbx` makes for [`fake_secret`] (`sbx-cloud-secret-set-custom.txt`).
const FAKE_NAME: &str = "postman-echo-com";

/// Asserts the fake value is in no log line, nothing printed, and no file of the task folder,
/// whether or not the folder still exists.
fn assert_value_nowhere(
    f: &common::task_fixture::Fixture,
    id: &str,
    backend: &sbxm::backend::FakeBackend,
    started: &Started,
) {
    assert!(
        !backend.log().join("\n").contains(FAKE_VALUE),
        "{:?}",
        backend.log()
    );
    assert!(!started.out.contains(FAKE_VALUE), "{}", started.out);
    assert!(!started.warn.contains(FAKE_VALUE), "{}", started.warn);
    if let Err(e) = &started.result {
        assert!(!format!("{e:#}").contains(FAKE_VALUE), "{e:#}");
    }
    let meta = sbxm::task::record::task_dir(&f.env.base_dir(), id);
    let mut pending = vec![meta];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if let Ok(bytes) = std::fs::read(&path) {
                assert!(
                    !String::from_utf8_lossy(&bytes).contains(FAKE_VALUE),
                    "{} holds the value",
                    path.display()
                );
            }
        }
    }
}

/// Boundary (one secret): `secret-set`, `create`, ..., the sandbox's `rm`, then the cloud's list
/// read and `secret-rm`, in that order, on a worker failure that stops the task; one line for the
/// removal; the value nowhere.
#[test]
fn one_scoped_secret_is_set_before_the_create_and_removed_after_the_sandbox() {
    let f = fixture_with(LOCAL_TOML);
    let id = spec_id(&f);
    let sandbox = format!("sbxm-task-{id}-claude");
    let backend = cloud_backend_with(&f.env.tmp.path().join("cloud-fs"), &id, failed_worker());

    let started = start_cloud_with_secrets(&f, &backend, &[fake_secret()]);

    assert!(started.result.is_err());
    let log = described_log(&backend);
    assert_eq!(
        log[0],
        format!("secret-set {sandbox} {FAKE_NAME}"),
        "{log:?}"
    );
    assert_eq!(log[1], format!("create {sandbox}"), "{log:?}");
    assert_eq!(
        log[log.len() - 3..],
        [
            format!("rm {sandbox}"),
            format!("secret-ls {sandbox}"),
            format!("secret-rm {sandbox} {FAKE_NAME}"),
        ],
        "{log:?}"
    );
    assert_eq!(
        backend.scoped_secret_sets(),
        [(sandbox.clone(), fake_secret())]
    );
    assert_eq!(backend.cloud_secrets(), Vec::<(String, String)>::new());
    assert_removed(&f, &id, &started, "removed");
    let line = format!("scoped secret {FAKE_NAME} of {sandbox} removed");
    assert_eq!(started.out.matches(&line).count(), 1, "{}", started.out);
    assert!(started.warn.is_empty(), "{}", started.warn);
    assert_value_nowhere(&f, &id, &backend, &started);
}

/// Boundary (no scoped secrets): no `secret-set`, no `secret-rm`, and the cloud's secret list is
/// never read, on a start that hands over and on one that stops.
#[test]
fn without_scoped_secrets_no_secret_call_is_made() {
    for worker in [
        common::task_fixture::ok(common::task_fixture::CLAUDE_DONE),
        failed_worker(),
    ] {
        let f = fixture_with(LOCAL_TOML);
        let id = spec_id(&f);
        let backend = cloud_backend_with(&f.env.tmp.path().join("cloud-fs"), &id, worker);

        let _ = start_cloud(&f, &backend);

        let secret_calls: Vec<_> = backend
            .log()
            .into_iter()
            .filter(|entry| entry.starts_with("secret-"))
            .collect();
        assert_eq!(secret_calls, Vec::<String>::new());
        assert!(backend.scoped_secret_sets().is_empty());
    }
}

/// A start that hands over to the review keeps its sandbox (slice 3), and its scoped secret with
/// it: the review and fix rounds run in that sandbox.
#[test]
fn a_start_that_hands_over_keeps_the_sandbox_and_its_scoped_secret() {
    let f = fixture_with(LOCAL_TOML);
    let id = spec_id(&f);
    let sandbox = format!("sbxm-task-{id}-claude");
    let backend = cloud_backend(&f.env.tmp.path().join("cloud-fs"), &id);

    let started = start_cloud_with_secrets(&f, &backend, &[fake_secret()]);

    started.result.as_ref().unwrap();
    assert!(backend.removes().is_empty());
    assert_eq!(
        backend.cloud_secrets(),
        [(sandbox.clone(), FAKE_NAME.to_owned())]
    );
    let log = described_log(&backend);
    assert_eq!(
        log[0],
        format!("secret-set {sandbox} {FAKE_NAME}"),
        "{log:?}"
    );
    assert!(
        !log.iter()
            .any(|e| e.starts_with("secret-rm") || e.starts_with("secret-ls")),
        "{log:?}"
    );
    assert_value_nowhere(&f, &id, &backend, &started);
}

/// Boundary (a failed `create`): `secret-set`, `create` (fails), the list read, `secret-rm`, then
/// the error; no sandbox `rm`, no task folder.
#[test]
fn a_failed_create_still_removes_the_scoped_secret_set_before_it() {
    let f = fixture_with(LOCAL_TOML);
    let id = spec_id(&f);
    let sandbox = format!("sbxm-task-{id}-claude");
    let backend = cloud_ready().with_failing_create_for(&sandbox);

    let started = start_cloud_with_secrets(&f, &backend, &[fake_secret()]);

    let message = format!("{:#}", started.result.as_ref().unwrap_err());
    assert!(message.contains(&sandbox), "{message}");
    assert_eq!(
        described_log(&backend),
        [
            format!("secret-set {sandbox} {FAKE_NAME}"),
            format!("create {sandbox}"),
            format!("secret-ls {sandbox}"),
            format!("secret-rm {sandbox} {FAKE_NAME}"),
        ]
    );
    assert!(backend.removes().is_empty());
    assert_eq!(backend.cloud_secrets(), Vec::<(String, String)>::new());
    assert!(!sbxm::task::record::task_dir(&f.env.base_dir(), &id).exists());
    let line = format!("scoped secret {FAKE_NAME} of {sandbox} removed");
    assert_eq!(started.out.matches(&line).count(), 1, "{}", started.out);
    assert!(started.warn.is_empty(), "{}", started.warn);
    assert_value_nowhere(&f, &id, &backend, &started);
}

/// Boundary (a failure after the create): the sandbox's removal, then the list read and
/// `secret-rm`; the folder goes with the sandbox.
#[test]
fn a_failure_after_the_create_removes_the_sandbox_then_the_scoped_secret() {
    let f = fixture_with(LOCAL_TOML);
    let id = spec_id(&f);
    let sandbox = format!("sbxm-task-{id}-claude");
    let backend =
        cloud_ready().with_exec_output_matching("printenv", common::task_fixture::ok("work\n"));

    let started = start_cloud_with_secrets(&f, &backend, &[fake_secret()]);

    let message = format!("{:#}", started.result.as_ref().unwrap_err());
    assert!(message.contains("HOME"), "{message}");
    assert_eq!(
        described_log(&backend),
        [
            format!("secret-set {sandbox} {FAKE_NAME}"),
            format!("create {sandbox}"),
            "exec printenv HOME".to_owned(),
            format!("rm {sandbox}"),
            format!("secret-ls {sandbox}"),
            format!("secret-rm {sandbox} {FAKE_NAME}"),
        ]
    );
    assert_eq!(backend.cloud_secrets(), Vec::<(String, String)>::new());
    assert!(!sbxm::task::record::task_dir(&f.env.base_dir(), &id).exists());
    assert_start_lines(&started, &sandbox, "removed");
    let line = format!("scoped secret {FAKE_NAME} of {sandbox} removed");
    assert_eq!(started.out.matches(&line).count(), 1, "{}", started.out);
    assert!(started.warn.is_empty(), "{}", started.warn);
    assert_value_nowhere(&f, &id, &backend, &started);
}

/// Boundary (a failed sandbox removal): the secrets are still tried, after the last `rm` attempt,
/// and both are reported, the sandbox as the warning that keeps the record, the secret as its
/// removed line. On a start that fails after its create and on a worker failure alike.
#[test]
fn a_failed_sandbox_removal_still_removes_the_scoped_secret_and_reports_both() {
    let f = fixture_with(LOCAL_TOML);
    let id = spec_id(&f);
    let sandbox = format!("sbxm-task-{id}-claude");
    let error = unavailable(&sandbox);
    let root = f.env.tmp.path().join("cloud-fs");
    let after_create =
        cloud_ready().with_exec_output_matching("printenv", common::task_fixture::ok("work\n"));
    let at_the_worker = cloud_backend_with(&root, &id, failed_worker());

    for backend in [after_create, at_the_worker] {
        let backend = backend.with_remove_errors(&[&error, &error, &error, &error]);
        let started = start_cloud_with_secrets(&f, &backend, &[fake_secret()]);

        assert!(started.result.is_err());
        let log = described_log(&backend);
        assert_eq!(
            log[log.len() - 6..],
            [
                format!("rm {sandbox}"),
                format!("rm {sandbox}"),
                format!("rm {sandbox}"),
                format!("rm {sandbox}"),
                format!("secret-ls {sandbox}"),
                format!("secret-rm {sandbox} {FAKE_NAME}"),
            ],
            "{log:?}"
        );
        assert_eq!(started.waits, secs(&[5, 10, 20]));
        assert_eq!(backend.cloud_secrets(), Vec::<(String, String)>::new());
        // Both reported: the sandbox as a warning, the secret as its line.
        let failed = format!("warning: cloud sandbox {sandbox} could not be removed: ");
        assert_eq!(started.warn.matches(&failed).count(), 1, "{}", started.warn);
        let line = format!("scoped secret {FAKE_NAME} of {sandbox} removed");
        assert_eq!(started.out.matches(&line).count(), 1, "{}", started.out);
        // The record stays for the sandbox that may still be there (slice 3), with nothing about
        // the secret in it: the cloud's list is its record.
        let task = task_record(&f, &id);
        assert_eq!(task.sandboxes[0].removed_at, None);
        assert_value_nowhere(&f, &id, &backend, &started);
        std::fs::remove_dir_all(sbxm::task::record::task_dir(&f.env.base_dir(), &id)).unwrap();
    }
}

/// A second secret for the tests, so an order and a failure in the middle can show.
fn second_secret() -> sbxm::backend::ScopedSecret {
    sbxm::backend::ScopedSecret {
        host: "api.example.com".into(),
        env: "SECOND_FAKE".into(),
        value: FAKE_VALUE.into(),
    }
}

const SECOND_NAME: &str = "api-example-com";

/// Boundary (another sandbox's secret in the cloud's list): left alone; only the task's own
/// secrets, in the list's order, go. With two of the task's: both set before the create, in
/// order, and both removed.
#[test]
fn only_the_secrets_scoped_to_the_task_sandbox_are_removed() {
    let f = fixture_with(LOCAL_TOML);
    let id = spec_id(&f);
    let sandbox = format!("sbxm-task-{id}-claude");
    let other = "sbxm-task-other-claude";
    let backend = cloud_backend_with(&f.env.tmp.path().join("cloud-fs"), &id, failed_worker())
        .with_cloud_secret(other, FAKE_NAME);

    let started = start_cloud_with_secrets(&f, &backend, &[fake_secret(), second_secret()]);

    assert!(started.result.is_err());
    let log = described_log(&backend);
    assert_eq!(
        log[..3],
        [
            format!("secret-set {sandbox} {FAKE_NAME}"),
            format!("secret-set {sandbox} {SECOND_NAME}"),
            format!("create {sandbox}"),
        ],
        "{log:?}"
    );
    assert_eq!(
        log[log.len() - 4..],
        [
            format!("rm {sandbox}"),
            format!("secret-ls {sandbox}"),
            format!("secret-rm {sandbox} {FAKE_NAME}"),
            format!("secret-rm {sandbox} {SECOND_NAME}"),
        ],
        "{log:?}"
    );
    assert_eq!(
        backend.cloud_secrets(),
        [(other.to_owned(), FAKE_NAME.to_owned())]
    );
    assert!(!started.out.contains(other), "{}", started.out);
    assert!(started.warn.is_empty(), "{}", started.warn);
    assert_value_nowhere(&f, &id, &backend, &started);
}

/// A set that fails in the middle: no create, the secrets that landed are removed (the cloud's
/// list says which), the error names the secret and the sandbox, never the value, and no task
/// folder stays.
#[test]
fn a_scoped_secret_that_cannot_be_set_stops_the_start_before_the_create() {
    let f = fixture_with(LOCAL_TOML);
    let id = spec_id(&f);
    let sandbox = format!("sbxm-task-{id}-claude");
    let backend = cloud_ready().failing_set_scoped_secret_for("api.example.com");

    let started = start_cloud_with_secrets(&f, &backend, &[fake_secret(), second_secret()]);

    let message = format!("{:#}", started.result.as_ref().unwrap_err());
    assert!(
        message.contains("SECOND_FAKE")
            && message.contains("api.example.com")
            && message.contains(&sandbox),
        "{message}"
    );
    assert_eq!(
        described_log(&backend),
        [
            format!("secret-set {sandbox} {FAKE_NAME}"),
            format!("secret-set {sandbox} {SECOND_NAME}"),
            format!("secret-ls {sandbox}"),
            format!("secret-rm {sandbox} {FAKE_NAME}"),
        ]
    );
    assert!(backend.creates().is_empty());
    assert_eq!(backend.cloud_secrets(), Vec::<(String, String)>::new());
    assert!(!sbxm::task::record::task_dir(&f.env.base_dir(), &id).exists());
    assert!(started.warn.is_empty(), "{}", started.warn);
    assert_value_nowhere(&f, &id, &backend, &started);
}

/// A secret list the cloud won't give: one warning naming `sbx --cloud secret ls` and the
/// sandbox, no `secret-rm`, and the task's own outcome unchanged.
#[test]
fn a_secret_list_that_cannot_be_read_is_one_warning_naming_the_command() {
    let f = fixture_with(LOCAL_TOML);
    let id = spec_id(&f);
    let sandbox = format!("sbxm-task-{id}-claude");
    let backend = cloud_backend_with(&f.env.tmp.path().join("cloud-fs"), &id, failed_worker())
        .failing_scoped_secrets();

    let started = start_cloud_with_secrets(&f, &backend, &[fake_secret()]);

    let message = format!("{:#}", started.result.as_ref().unwrap_err());
    assert!(message.contains("worker"), "{message}");
    let log = described_log(&backend);
    assert_eq!(
        log[log.len() - 2..],
        [format!("rm {sandbox}"), format!("secret-ls {sandbox}")],
        "{log:?}"
    );
    assert_removed(&f, &id, &started, "removed");
    let warning = format!(
        "warning: the cloud's secret list couldn't be read, so any secret scoped to {sandbox} is \
         still there: "
    );
    assert_eq!(
        started.warn.matches(&warning).count(),
        1,
        "{}",
        started.warn
    );
    assert!(
        started.warn.contains("`sbx --cloud secret ls`")
            && started.warn.contains(&format!("--sandbox {sandbox} -f")),
        "{}",
        started.warn
    );
    assert!(!started.out.contains("scoped secret"), "{}", started.out);
    assert_value_nowhere(&f, &id, &backend, &started);
}

/// A secret the cloud won't remove: one warning naming the command that removes it by hand; the
/// others still go; the folder doesn't stay for it (`task.json` records no secret).
#[test]
fn a_scoped_secret_that_cannot_be_removed_is_one_warning_naming_the_command() {
    let f = fixture_with(LOCAL_TOML);
    let id = spec_id(&f);
    let sandbox = format!("sbxm-task-{id}-claude");
    let backend = cloud_ready()
        .with_exec_output_matching("printenv", common::task_fixture::ok("work\n"))
        .failing_remove_secret_for(FAKE_NAME);

    let started = start_cloud_with_secrets(&f, &backend, &[fake_secret(), second_secret()]);

    assert!(started.result.is_err());
    let log = described_log(&backend);
    assert_eq!(
        log[log.len() - 4..],
        [
            format!("rm {sandbox}"),
            format!("secret-ls {sandbox}"),
            format!("secret-rm {sandbox} {FAKE_NAME}"),
            format!("secret-rm {sandbox} {SECOND_NAME}"),
        ],
        "{log:?}"
    );
    assert_eq!(
        backend.cloud_secrets(),
        [(sandbox.clone(), FAKE_NAME.to_owned())]
    );
    let warning = format!("warning: scoped secret {FAKE_NAME} of {sandbox} could not be removed: ");
    assert_eq!(
        started.warn.matches(&warning).count(),
        1,
        "{}",
        started.warn
    );
    let command = format!("sbx --cloud secret rm {FAKE_NAME} --sandbox {sandbox} -f");
    assert!(started.warn.contains(&command), "{}", started.warn);
    let line = format!("scoped secret {SECOND_NAME} of {sandbox} removed");
    assert_eq!(started.out.matches(&line).count(), 1, "{}", started.out);
    assert!(!sbxm::task::record::task_dir(&f.env.base_dir(), &id).exists());
    assert_value_nowhere(&f, &id, &backend, &started);
}

/// Self-review (check before acting): the list is an input, so its shape is refused before any
/// write or changing backend call, naming the field and never the value; and a local start never
/// drops a list silently.
#[test]
fn a_malformed_or_misplaced_scoped_secret_is_refused_before_anything() {
    let f = fixture_with(LOCAL_TOML);
    let id = spec_id(&f);
    let bad = |host: &str, env: &str, value: &str| sbxm::backend::ScopedSecret {
        host: host.into(),
        env: env.into(),
        value: value.into(),
    };
    let cases: [(sbxm::backend::ScopedSecret, &str); 5] = [
        (bad("", "P2_FAKE", FAKE_VALUE), "host"),
        (bad("postman echo.com", "P2_FAKE", FAKE_VALUE), "host"),
        (bad("postman-echo.com", "", FAKE_VALUE), "env"),
        (bad("postman-echo.com", "2BAD", FAKE_VALUE), "env"),
        (bad("postman-echo.com", "P2_FAKE", ""), "value"),
    ];
    for (secret, field) in cases {
        let backend = cloud_ready();
        let started = start_cloud_with_secrets(&f, &backend, &[fake_secret(), secret]);
        let message = format!("{:#}", started.result.as_ref().unwrap_err());
        assert!(
            message.contains(field) && message.contains("scoped secret 2"),
            "{message}"
        );
        assert!(!message.contains(FAKE_VALUE), "{message}");
        assert!(backend.log().is_empty(), "{:?}", backend.log());
        assert!(!f.env.base_dir().join(".sbxm").join("tasks").exists());
    }

    // A local start with scoped secrets: refused, not ignored (decision 11).
    let backend = cloud_ready();
    let spec = common::task_fixture::spec_file(&f, "Build a thing.\n");
    let result = sbxm::commands::task_start::run_spec_with_seams(
        &f.env.config_dir(),
        &spec_options(&f, spec, Some(Location::Local)),
        &backend,
        &sbxm::github::fake::FakeGitHub::default(),
        &common::task_fixture::Probe,
        &mut Vec::new(),
        &mut Vec::new(),
        &std::thread::sleep,
        &[fake_secret()],
    );
    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("scoped secret") && message.contains("cloud"),
        "{message}"
    );
    assert!(!message.contains(FAKE_VALUE), "{message}");
    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert!(!sbxm::task::record::task_dir(&f.env.base_dir(), &id).exists());
}

/// P14 (decision 262(k)): `task run --issue --local` overrides a cloud `[sandbox] location` for
/// the whole command, the start included: it asks GitHub for the issue, with no cloud call.
#[test]
fn task_run_issue_local_overrides_a_cloud_config_for_the_start_too() {
    use sbxm::commands::task_run::{self, Options};

    let f = fixture_with(CLOUD_TOML);
    let backend = sbxm::backend::FakeBackend::with_secrets(&["anthropic", "openai"]);
    let github = sbxm::github::fake::FakeGitHub::default();
    let result = task_run::run(
        &f.env.config_dir(),
        &Options {
            repo_root: f.env.tmp.path().join("target-repo"),
            issue: 41,
            worker_harness: None,
            worker_model: None,
            reviewer_harness: None,
            reviewer_model: None,
            time_limit: None,
            reviewer_time_limit: None,
            profile: None,
            base: Some("main".into()),
            repo: Some("o/r".into()),
            clone_source: Some(common::task_fixture::source(&f)),
            identity: Some(identity()),
            disk: Default::default(),
            location: Some(Location::Local),
        },
        None,
        &backend,
        &github,
        &common::task_fixture::Probe,
        &sbxm::task::gates::FakeHostRunner::default(),
        &mut Vec::new(),
        &mut Vec::new(),
    );

    if let Err(e) = result {
        let message = format!("{e:#}");
        assert!(!message.contains("cloud"), "{message}");
    }
    assert!(!github.calls().is_empty());
    assert!(
        !backend.locations().contains(&Location::Cloud),
        "{:?}",
        backend.locations()
    );
}

// Slice 7: strict mode in the cloud (`sdlc/milestone-4.md` P9, P10, "After create: the
// strict-policy check fails closed"; decisions 255, 264). A strict sandbox is created with the
// build allow list, every allowed host outside the runtime list (the harness's provider hosts plus
// `[template].allow`) is denied in it, the denied hosts go to `run.log`, and the list read again
// must hold only the runtime list before any agent `exec`.

/// The claude harness's runtime hosts (P9 (a)).
const CLAUDE_HOSTS: [&str; 3] = ["api.anthropic.com", "claude.ai", "console.anthropic.com"];

/// The rest of the claude agent's built-in list as S17 saw it, none of them a runtime host.
const BUILT_IN_OTHERS: [&str; 6] = [
    "api.github.com",
    "github.com",
    "api.mistral.ai",
    "api.openai.com",
    "openrouter.ai",
    "api.x.ai",
];

const STRICT_PROFILE: &str =
    "[template]\nstrict = true\nallow = [\"crates.io\", \"*.example.org\"]\n";

/// The `run.log` line (decision 264) of a strict cloud sandbox's denies.
fn denied_line(sandbox: &str, hosts: &[&str]) -> String {
    if hosts.is_empty() {
        format!("strict cloud sandbox {sandbox}: sbxm denied no hosts")
    } else {
        format!(
            "strict cloud sandbox {sandbox}: sbxm denied {}",
            hosts.join(", ")
        )
    }
}

/// Boundary: a list that is exactly the runtime list passes with no deny, and the agent runs.
#[test]
fn a_strict_cloud_worker_whose_list_is_the_runtime_list_runs_the_agent() {
    let f = fixture_with(LOCAL_TOML);
    f.env.write_profile("default", STRICT_PROFILE);
    let id = spec_id(&f);
    let sandbox = format!("sbxm-task-{id}-claude");
    let mut runtime = CLAUDE_HOSTS.to_vec();
    runtime.push("crates.io");
    let backend = cloud_backend(&f.env.tmp.path().join("cloud-fs"), &id).with_cloud_allow(&runtime);

    let (result, out) = start_spec(&f, &backend, Some(Location::Cloud));

    result.unwrap();
    let log = described_log(&backend);
    assert_eq!(
        log[..4],
        [
            format!("create {sandbox}"),
            format!("policy-ls {sandbox}"),
            format!("policy-ls {sandbox}"),
            "exec printenv HOME".to_owned(),
        ]
    );
    assert!(log.contains(&"exec worker".to_owned()), "{log:?}");
    assert!(!log.iter().any(|e| e.starts_with("policy-deny")), "{log:?}");
    assert_eq!(out.matches(&denied_line(&sandbox, &[])).count(), 1, "{out}");
    // Created from the profile's kits with the build allow list: no template, no create-time deny.
    let create = &backend.creates()[0];
    assert_eq!(create.template, None);
    assert!(create.deny_network.is_empty());
}

/// The S17-shape list of the claude agent's built-in hosts plus a build host: every host outside
/// the runtime list is denied, in the list's order; `[template].allow` hosts, exact and through
/// `*.`, are kept; one line names the denied hosts; then the list is checked and the agent runs.
#[test]
fn a_strict_cloud_worker_denies_every_host_outside_the_runtime_list_and_keeps_the_allowed_ones() {
    let f = fixture_with(LOCAL_TOML);
    f.env.write_profile("default", STRICT_PROFILE);
    let id = spec_id(&f);
    let sandbox = format!("sbxm-task-{id}-claude");
    let mut list = CLAUDE_HOSTS.to_vec();
    list.extend(BUILT_IN_OTHERS);
    list.extend([
        "crates.io",
        "static.example.org",
        "example.org",
        "postman-echo.com",
    ]);
    let backend = cloud_backend(&f.env.tmp.path().join("cloud-fs"), &id).with_cloud_allow(&list);

    let (result, out) = start_spec(&f, &backend, Some(Location::Cloud));

    result.unwrap();
    // `*.example.org` covers hosts under the domain, not the domain itself (as [232]).
    let mut denied = BUILT_IN_OTHERS.to_vec();
    denied.extend(["example.org", "postman-echo.com"]);
    let mut expected = vec![format!("create {sandbox}"), format!("policy-ls {sandbox}")];
    expected.extend(denied.iter().map(|h| format!("policy-deny {sandbox} {h}")));
    expected.extend([
        format!("policy-ls {sandbox}"),
        "exec printenv HOME".to_owned(),
    ]);
    let log = described_log(&backend);
    assert_eq!(log[..expected.len()], expected[..], "{log:?}");
    assert!(log.contains(&"exec worker".to_owned()), "{log:?}");
    assert_eq!(
        out.matches(&denied_line(&sandbox, &denied)).count(),
        1,
        "{out}"
    );
}

/// The `codex` harness keeps decision 221's six OpenAI hosts.
#[test]
fn a_strict_cloud_codex_worker_keeps_the_openai_hosts() {
    let f = fixture_with(LOCAL_TOML);
    f.env.write_profile("default", STRICT_PROFILE);
    let id = spec_id(&f);
    let sandbox = format!("sbxm-task-{id}-codex");
    let openai = [
        "api.openai.com",
        "openai.com",
        "auth.openai.com",
        "chatgpt.com",
        "files.openai.com",
        "releases.openai.com",
    ];
    let mut list = openai.to_vec();
    list.extend(["github.com", "api.anthropic.com"]);
    let backend = cloud_ready()
        .and_secrets(&["openai"])
        .with_cloud_allow(&list);
    let spec = common::task_fixture::spec_file(&f, "Build a thing.\n");
    let mut options = spec_options(&f, spec, Some(Location::Cloud));
    options.worker_harness = Some(sbxm::harness::Harness::Codex);
    let mut out = Vec::new();

    // The fake's `printenv HOME` answers nothing, so the start fails after the check passed.
    let _ = sbxm::commands::task_start::run_spec(
        &f.env.config_dir(),
        &options,
        &backend,
        &sbxm::github::fake::FakeGitHub::default(),
        &common::task_fixture::Probe,
        &mut out,
        &mut Vec::new(),
    );

    let out = String::from_utf8(out).unwrap();
    let log = described_log(&backend);
    assert_eq!(
        log[..5],
        [
            format!("create {sandbox}"),
            format!("policy-ls {sandbox}"),
            format!("policy-deny {sandbox} github.com"),
            format!("policy-deny {sandbox} api.anthropic.com"),
            format!("policy-ls {sandbox}"),
        ],
        "{log:?}"
    );
    assert_eq!(log[5], "exec printenv HOME", "{log:?}");
    assert!(
        out.contains(&denied_line(&sandbox, &["github.com", "api.anthropic.com"])),
        "{out}"
    );
}

/// Asserts the fail-closed order (behavior 4): `secret-set`, `create`, the first `policy ls`, the
/// denies, the `policy ls` that fails the check, the sandbox's `rm`, the cloud's secret list and
/// `secret-rm`; no `exec` at all, so no agent ran. The task folder goes with the sandbox, as a
/// local strict `codex` start that [232] refuses leaves it.
fn assert_failed_closed(
    f: &common::task_fixture::Fixture,
    id: &str,
    backend: &sbxm::backend::FakeBackend,
    started: &Started,
    denies: &[&str],
    reads: usize,
) {
    let sandbox = format!("sbxm-task-{id}-claude");
    let mut expected = vec![
        format!("secret-set {sandbox} {FAKE_NAME}"),
        format!("create {sandbox}"),
        format!("policy-ls {sandbox}"),
    ];
    expected.extend(denies.iter().map(|h| format!("policy-deny {sandbox} {h}")));
    if reads == 2 {
        expected.push(format!("policy-ls {sandbox}"));
    }
    expected.extend([
        format!("rm {sandbox}"),
        format!("secret-ls {sandbox}"),
        format!("secret-rm {sandbox} {FAKE_NAME}"),
    ]);
    assert_eq!(described_log(backend), expected);
    assert!(backend.execs().is_empty());
    assert_eq!(backend.cloud_secrets(), Vec::<(String, String)>::new());
    assert!(!sbxm::task::record::task_dir(&f.env.base_dir(), id).exists());
    assert_start_lines(started, &sandbox, "removed");
    let line = format!("scoped secret {FAKE_NAME} of {sandbox} removed");
    assert_eq!(started.out.matches(&line).count(), 1, "{}", started.out);
    // The removal lines come before the refusal: the refusal is the command's result.
    assert!(started.result.is_err());
}

/// Boundary: one extra host still allowed after the denies (a rule sbxm's deny doesn't win over):
/// the sandbox goes, then its scoped secret, then the refusal names the host; no agent `exec`.
#[test]
fn a_host_left_after_the_denies_removes_the_sandbox_then_the_secret_then_refuses() {
    let f = fixture_with(LOCAL_TOML);
    f.env.write_profile("default", STRICT_PROFILE);
    let id = spec_id(&f);
    let sandbox = format!("sbxm-task-{id}-claude");
    let mut list = CLAUDE_HOSTS.to_vec();
    list.extend(["api.x.ai", "github.com"]);
    let backend = cloud_ready()
        .with_cloud_allow(&list)
        .with_undeniable_cloud_host("api.x.ai");

    let started = start_cloud_with_secrets(&f, &backend, &[fake_secret()]);

    assert_failed_closed(&f, &id, &backend, &started, &["api.x.ai", "github.com"], 2);
    let message = format!("{:#}", started.result.as_ref().unwrap_err());
    assert!(
        message.contains("api.x.ai") && message.contains(&sandbox) && message.contains("strict"),
        "{message}"
    );
    assert!(!message.contains("github.com"), "{message}");
    // The denied hosts were written before the check failed.
    assert_eq!(
        started
            .out
            .matches(&denied_line(&sandbox, &["api.x.ai", "github.com"]))
            .count(),
        1,
        "{}",
        started.out
    );
    assert_value_nowhere(&f, &id, &backend, &started);
}

/// Boundary: a `policy ls` sbxm can't read or parse, after the denies or before them, takes the
/// same fail-closed path, naming the unread list.
#[test]
fn an_unreadable_policy_list_removes_the_sandbox_then_the_secret_then_refuses() {
    for (from, denies, reads) in [(1, vec!["github.com"], 2), (0, vec![], 1)] {
        let f = fixture_with(LOCAL_TOML);
        f.env.write_profile("default", STRICT_PROFILE);
        let id = spec_id(&f);
        let sandbox = format!("sbxm-task-{id}-claude");
        let mut list = CLAUDE_HOSTS.to_vec();
        list.push("github.com");
        let backend = cloud_ready()
            .with_cloud_allow(&list)
            .failing_cloud_policy_ls_from(from);

        let started = start_cloud_with_secrets(&f, &backend, &[fake_secret()]);

        assert_failed_closed(&f, &id, &backend, &started, &denies, reads);
        let message = format!("{:#}", started.result.as_ref().unwrap_err());
        assert!(
            message.contains("can't read")
                && message.contains(&format!("sbx --cloud policy ls {sandbox} --json")),
            "{message}"
        );
        let line = denied_line(&sandbox, &denies);
        assert_eq!(
            started.out.matches(&line).count(),
            reads - 1,
            "{}",
            started.out
        );
    }
}

/// A strict profile with a harness sbxm has no provider list for is refused, naming the harness;
/// claude and codex have one, and a non-strict profile needs none. Unit level: of those harnesses
/// a task can't run `pi` at all, and the cloud preflight refuses `antigravity`'s google login
/// first.
#[test]
fn a_strict_profile_with_an_unsupported_harness_is_refused_for_the_cloud() {
    use sbxm::harness::Harness;

    let f = fixture_with(LOCAL_TOML);
    f.env.write_profile("default", STRICT_PROFILE);
    f.env
        .write_profile("loose", "[template]\nallow = [\"crates.io\"]\n");
    let strict = sbxm::config::Profile::load(&f.env.profiles_dir(), "default").unwrap();
    let loose = sbxm::config::Profile::load(&f.env.profiles_dir(), "loose").unwrap();

    for harness in [Harness::Pi, Harness::Antigravity, Harness::Gemini] {
        let message = format!(
            "{:#}",
            sbxm::template::check_strict_cloud("default", &strict, harness).unwrap_err()
        );
        assert!(
            message.contains("strict")
                && message.contains(&format!("the {} harness", harness.as_str()))
                && message.contains("--local"),
            "{message}"
        );
        sbxm::template::check_strict_cloud("loose", &loose, harness).unwrap();
    }
    for harness in [Harness::Claude, Harness::Codex] {
        sbxm::template::check_strict_cloud("default", &strict, harness).unwrap();
    }
}
