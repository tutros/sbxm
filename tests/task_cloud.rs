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

fn assert_refused_before_anything(
    result: anyhow::Result<()>,
    f: &common::task_fixture::Fixture,
    backend: &sbxm::backend::FakeBackend,
    github: &sbxm::github::fake::FakeGitHub,
    command: &str,
) {
    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains(command) && message.contains("cloud"),
        "{message}"
    );
    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert!(github.calls().is_empty(), "{:?}", github.calls());
    assert!(!f.env.base_dir().join(".sbxm").join("tasks").exists());
}

/// Decision 261: a command whose sandboxes can't run in the cloud yet refuses a cloud location
/// before anything, instead of running them here.
#[test]
fn an_issue_start_in_a_cloud_repo_is_refused_before_anything_and_local_overrides_it() {
    use sbxm::commands::task_start::{self, Options};

    let f = fixture_with(CLOUD_TOML);
    let options = |location| Options {
        repo_root: f.env.tmp.path().join("target-repo"),
        issues: vec![41],
        workers: None,
        worker_harness: None,
        worker_model: None,
        time_limit: None,
        profile: None,
        base: Some("main".into()),
        repo: Some("o/r".into()),
        clone_source: Some(common::task_fixture::source(&f)),
        identity: Some(identity()),
        disk: Default::default(),
        location,
    };
    let run = |opts: &Options,
               backend: &sbxm::backend::FakeBackend,
               github: &sbxm::github::fake::FakeGitHub| {
        task_start::run_with(
            &f.env.config_dir(),
            opts,
            None,
            backend,
            github,
            &common::task_fixture::Probe,
            &mut Vec::new(),
            &mut Vec::new(),
        )
    };

    for location in [None, Some(Location::Cloud)] {
        let backend = common::task_fixture::backend();
        let github = sbxm::github::fake::FakeGitHub::default();
        let result = run(&options(location), &backend, &github);
        assert_refused_before_anything(result, &f, &backend, &github, "task start --issue");
    }

    // `--local` gets past it: the start goes on to ask GitHub for the issue.
    let backend = common::task_fixture::backend();
    let github = sbxm::github::fake::FakeGitHub::default();
    let _ = run(&options(Some(Location::Local)), &backend, &github);
    assert!(!github.calls().is_empty());
}

#[test]
fn task_run_in_a_cloud_repo_is_refused_before_anything() {
    use sbxm::commands::task_run::{self, SpecOptions};

    let f = fixture_with(CLOUD_TOML);
    let spec = common::task_fixture::spec_file(&f, "Build a thing.\n");
    let backend = common::task_fixture::backend();
    let github = sbxm::github::fake::FakeGitHub::default();
    let result = task_run::run_spec(
        &f.env.config_dir(),
        &SpecOptions {
            repo_root: f.env.tmp.path().join("target-repo"),
            spec,
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
        },
        None,
        &backend,
        &github,
        &common::task_fixture::Probe,
        &sbxm::task::gates::FakeHostRunner::default(),
        &mut Vec::new(),
        &mut Vec::new(),
    );

    assert_refused_before_anything(result, &f, &backend, &github, "task run");
}

#[test]
fn a_pr_review_in_a_cloud_repo_is_refused_before_anything() {
    use sbxm::commands::task_review::{self, Options, Target};

    let f = fixture_with(CLOUD_TOML);
    let backend = common::task_fixture::backend();
    let github = sbxm::github::fake::FakeGitHub::default();
    let result = task_review::run(
        &f.env.config_dir(),
        &Options {
            repo_root: f.env.tmp.path().join("target-repo"),
            target: Target::Pr(7),
            repo: Some("o/r".into()),
            base: Some("main".into()),
            clone_source: Some(common::task_fixture::source(&f)),
            reviewer_harness: None,
            reviewer_model: None,
            reviewer_time_limit: None,
            time_limit: None,
            profile: None,
            resolved_spec_id: None,
            force: false,
            disk: Default::default(),
        },
        &backend,
        &github,
        &common::task_fixture::Probe,
        &sbxm::task::gates::FakeHostRunner::default(),
        &mut Vec::new(),
        &mut Vec::new(),
    );

    assert_refused_before_anything(result, &f, &backend, &github, "task review --pr");
}

/// The cloud sandbox's home, as `printenv HOME` reports it in the fake.
const HOME: &str = "/home/agent";

/// A fake cloud: `<root>` stands in for the sandbox's filesystem (copy in and out land there),
/// `printenv HOME` answers [`HOME`], and the sandbox's own `sh`, `git` and `mkdir` commands really
/// run on the host with their in-sandbox paths moved under `<root>`. The worker's headless run
/// commits `a.txt` in its clone and writes `result.md`; every other command (the gates) passes.
fn cloud_backend(root: &std::path::Path, id: &str) -> sbxm::backend::FakeBackend {
    use common::task_fixture::{CLAUDE_DONE, is_headless, ok};

    std::fs::create_dir_all(root.join(HOME.trim_start_matches('/'))).unwrap();
    let inside = root.to_string_lossy().into_owned();
    let map = move |arg: &str| arg.replace(HOME, &format!("{inside}{HOME}"));
    let worker_map = map.clone();
    let clone = format!("{HOME}/work/{id}");
    common::task_fixture::backend()
        .with_sandbox_root(root)
        .with_exec_output_matching("printenv", ok(&format!("{HOME}\n")))
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
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

/// Only slice 7 makes strict cloud sandboxes: refused before anything is written or created.
#[test]
fn a_strict_profile_is_refused_for_the_cloud_before_anything() {
    let f = fixture_with(LOCAL_TOML);
    f.env.write_profile(
        "default",
        "[template]\nstrict = true\nallow = [\"crates.io\"]\n",
    );
    let backend = cloud_backend(&f.env.tmp.path().join("cloud-fs"), &spec_id(&f));

    let (result, _) = start_spec(&f, &backend, Some(Location::Cloud));

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("strict") && message.contains("slice 7") && message.contains("--local"),
        "{message}"
    );
    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert!(!f.env.base_dir().join(".sbxm").join("tasks").exists());
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
    let backend = common::task_fixture::backend()
        .with_exec_output_matching("printenv", common::task_fixture::ok("work\n"));

    let (result, _) = start_spec(&f, &backend, Some(Location::Cloud));

    let message = format!("{:#}", result.unwrap_err());
    assert!(message.contains("HOME"), "{message}");
    let sandbox = format!("sbxm-task-{id}-claude");
    assert_eq!(backend.removes(), [sandbox]);
    assert!(!sbxm::task::record::task_dir(&f.env.base_dir(), &id).exists());
    assert!(!backend.log().iter().any(|e| e.starts_with("cp-in")));
}
