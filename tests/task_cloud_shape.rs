//! Issue 559: a cloud task's sandboxes get a billable shape. `[sandbox] cloud_shape` in
//! `sbxm-task.toml` names one of five (a fixed table, matched case-insensitively, refused at load
//! otherwise); unset, the local cpus/memory are used when they are exactly a shape, and a cloud
//! task is refused before anything is created when they aren't. Local sandboxes keep
//! `[resources]` and `[sandbox] cpus / memory`. Through `FakeBackend`.

mod common;

use common::task_fixture::fixture_with;
use sbxm::task::config::TaskConfig;
use sbxm::task::shape::Shape;

fn toml_with_shape(shape: &str) -> String {
    format!(
        "[sandbox]\nprofile = \"default\"\ncloud_shape = {shape:?}\n\n[gates]\nsandbox = [\"cargo test\"]\n"
    )
}

fn load(toml: &str) -> anyhow::Result<TaskConfig> {
    let f = fixture_with("[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n");
    let repo = f.env.tmp.path().join("target-repo");
    std::fs::write(repo.join("sbxm-task.toml"), toml).unwrap();
    TaskConfig::load(&repo)
}

// The key at load.

#[test]
fn each_of_the_five_names_loads_as_its_shape_with_the_table_s_cpus_and_memory() {
    let table = [
        ("micro", Shape::Micro, 1, "2g"),
        ("small", Shape::Small, 2, "4g"),
        ("medium", Shape::Medium, 4, "8g"),
        ("large", Shape::Large, 8, "16g"),
        ("xl", Shape::Xl, 16, "32g"),
    ];
    for (name, shape, cpus, memory) in table {
        let config = load(&toml_with_shape(name)).unwrap();
        assert_eq!(config.sandbox.cloud_shape, Some(shape), "{name}");
        assert_eq!(shape.cpus(), cpus, "{name}");
        assert_eq!(shape.memory(), memory, "{name}");
    }
}

#[test]
fn a_name_is_matched_case_insensitively() {
    let config = load(&toml_with_shape("Large")).unwrap();
    assert_eq!(config.sandbox.cloud_shape, Some(Shape::Large));
    let config = load(&toml_with_shape("XL")).unwrap();
    assert_eq!(config.sandbox.cloud_shape, Some(Shape::Xl));
}

#[test]
fn an_unknown_or_empty_name_is_refused_at_load_naming_the_five() {
    for name in ["huge", ""] {
        let message = format!("{:#}", load(&toml_with_shape(name)).unwrap_err());
        assert!(message.contains("sandbox.cloud_shape"), "{message}");
        assert!(
            message.contains("micro, small, medium, large or xl"),
            "{message}"
        );
    }
}

#[test]
fn the_key_absent_loads_as_none() {
    let config = load("[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n").unwrap();
    assert_eq!(config.sandbox.cloud_shape, None);
}

// A cloud task's sandboxes.

use common::cloud_task::{
    CLEAN, ONE, cloud_backend, cloud_backend_listing, reviewer_sandbox, spec, task_record,
    task_run, worker_sandbox,
};
use common::task_fixture::Fixture;
use sbxm::backend::{FakeBackend, Location};

/// The global config's `[resources]` (the fixture's are 4 CPUs / 8g) is now `cpus` / `memory`.
fn set_global_resources(f: &Fixture, cpus: u32, memory: &str) {
    let path = f.env.config_dir().join("config.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    let changed = text.replace(
        "[resources]\ncpus = 4\nmemory = \"8g\"\n",
        &format!("[resources]\ncpus = {cpus}\nmemory = {memory:?}\n"),
    );
    assert!(text.contains("[resources]\ncpus = 4\n"), "{text}");
    std::fs::write(&path, changed).unwrap();
}

/// The cpus and memory of every create, in order.
fn shapes(backend: &FakeBackend) -> Vec<(String, u32, String)> {
    backend
        .creates()
        .into_iter()
        .map(|c| (c.name, c.cpus, c.memory))
        .collect()
}

const NO_SHAPE_TOML: &str =
    "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n";

/// A cloud `task run --spec` under `toml` with the global `[resources]` at `cpus` / `memory`,
/// reviewed clean.
fn cloud_run(toml: &str, cpus: u32, memory: &str) -> (Fixture, String, FakeBackend, Ran) {
    let f = fixture_with(toml);
    set_global_resources(&f, cpus, memory);
    let (spec, id) = spec(&f);
    let backend = cloud_backend(&f.env.tmp.path().join("cloud-fs"), &id, &[CLEAN]);
    let ran = task_run(&f, spec, Some(Location::Cloud), &backend);
    (f, id, backend, ran)
}

use common::cloud_task::Ran;

#[test]
fn unset_local_resources_that_are_a_shape_size_the_cloud_sandboxes() {
    for (cpus, memory, shape) in [
        (8, "16g", Shape::Large),
        (2, "4g", Shape::Small),
        (4, "8g", Shape::Medium),
        (4, "8192m", Shape::Medium),
    ] {
        let (f, id, backend, ran) = cloud_run(NO_SHAPE_TOML, cpus, memory);
        ran.result.expect(&ran.out);
        let want = |name: String| (name, shape.cpus(), shape.memory());
        assert_eq!(
            shapes(&backend),
            vec![want(worker_sandbox(&id)), want(reviewer_sandbox(&id))],
            "{cpus} / {memory}"
        );
        let worker = task_record(&f, &id).worker.unwrap();
        assert_eq!(
            (worker.cpus, worker.memory),
            (Some(shape.cpus()), Some(shape.memory()))
        );
    }
}

#[test]
fn unset_local_resources_that_are_no_shape_refuse_a_cloud_task_before_anything() {
    let (f, id, backend, ran) = cloud_run(NO_SHAPE_TOML, 8, "4g");
    let message = format!("{:#}", ran.result.unwrap_err());
    assert!(
        message.contains(
            "8 CPUs / 4g isn't a cloud shape; set [sandbox] cloud_shape in sbxm-task.toml to \
             micro, small, medium, large or xl"
        ),
        "{message}"
    );
    assert!(backend.creates().is_empty(), "{:?}", backend.log());
    assert!(!sbxm::task::record::task_dir(&f.env.base_dir(), &id).exists());
}

#[test]
fn sandbox_cpus_and_memory_count_as_the_local_resources_when_the_key_is_unset() {
    let toml = "[sandbox]\nprofile = \"default\"\ncpus = 1\nmemory = \"2g\"\n\n[gates]\nsandbox = [\"cargo test\"]\n";
    let (_, id, backend, ran) = cloud_run(toml, 8, "4g");
    ran.result.expect(&ran.out);
    assert_eq!(
        shapes(&backend)[0],
        (worker_sandbox(&id), 1, "2g".to_owned())
    );
}

/// The key wins over `[resources]` and `[sandbox] cpus / memory`, for the worker, the reviewer
/// and a fix round's remade worker sandbox alike.
#[test]
fn the_key_sizes_the_worker_reviewer_and_fix_round_sandboxes() {
    let toml = "[sandbox]\nprofile = \"default\"\ncpus = 4\nmemory = \"4g\"\ncloud_shape = \"Large\"\n\n\
                [worker]\nfix_rounds = 1\n\n[gates]\nsandbox = [\"cargo test\"]\n";
    let f = fixture_with(toml);
    set_global_resources(&f, 3, "5g");
    let (spec, id) = spec(&f);
    // The worker sandbox isn't listed, so the fix round makes it again.
    let backend = cloud_backend_listing(
        &f.env.tmp.path().join("cloud-fs"),
        &id,
        &[ONE, CLEAN],
        false,
    );

    let ran = task_run(&f, spec, Some(Location::Cloud), &backend);

    ran.result.expect(&ran.out);
    let creates = shapes(&backend);
    let workers = creates
        .iter()
        .filter(|(name, _, _)| *name == worker_sandbox(&id))
        .count();
    assert_eq!(workers, 2, "{creates:?}");
    assert!(
        creates
            .iter()
            .any(|(name, _, _)| *name == reviewer_sandbox(&id)),
        "{creates:?}"
    );
    assert!(
        creates
            .iter()
            .all(|(_, cpus, memory)| (*cpus, memory.as_str()) == (8, "16g")),
        "{creates:?}"
    );
    let worker = task_record(&f, &id).worker.unwrap();
    assert_eq!((worker.cpus, worker.memory), (Some(8), Some("16g".into())));
}

/// A local task never reads the key: its sandbox keeps `[sandbox] cpus / memory`.
#[test]
fn a_local_task_keeps_its_local_resources_whatever_the_key_says() {
    let toml = "[sandbox]\nprofile = \"default\"\ncpus = 8\nmemory = \"4g\"\ncloud_shape = \"xl\"\n\n\
                [gates]\nsandbox = [\"cargo test\"]\n";
    let f = fixture_with(toml);
    let backend = common::task_fixture::backend();
    let github = sbxm::github::fake::FakeGitHub::default();
    let source = common::task_fixture::source(&f);
    let ctx = common::task_fixture::ctx(&f, &source, &backend, &github);

    sbxm::task::pipeline::prepare(&ctx, &common::task_fixture::issue_text(41)).unwrap();

    let create = &backend.creates()[0];
    assert_eq!((create.cpus, create.memory.as_str()), (8, "4g"));
}

/// The process that ran the task is gone, as it is for every resume.
struct Gone;

impl sbxm::task::record::ProcessProbe for Gone {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

/// A cloud task made before the key (recorded 8 / 16g) is resumed after `cloud_shape` says
/// small: its gone worker sandbox is made again with the recorded values, not the key's.
#[test]
fn resume_rebuilds_a_cloud_worker_with_its_recorded_resources_not_the_key() {
    let f = fixture_with(NO_SHAPE_TOML);
    set_global_resources(&f, 8, "16g");
    let (spec, id) = spec(&f);
    let root = f.env.tmp.path().join("cloud-fs");
    let failed = sbxm::backend::ExecOutput {
        stdout: String::new(),
        stderr: "boom\n".into(),
        exit_code: Some(1),
    };
    let backend = common::cloud_task::cloud_backend_worker(&root, &[&id], &[CLEAN], true, failed);
    let ran = task_run(&f, spec.clone(), Some(Location::Cloud), &backend);
    assert!(ran.result.is_err(), "{}", ran.out);
    let worker = task_record(&f, &id).worker.unwrap();
    assert_eq!((worker.cpus, worker.memory), (Some(8), Some("16g".into())));
    std::fs::write(
        f.env.tmp.path().join("target-repo").join("sbxm-task.toml"),
        toml_with_shape("small"),
    )
    .unwrap();
    let gone = cloud_backend_listing(&root, &id, &[CLEAN], false);

    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = sbxm::commands::task_resume::run(
        &f.env.config_dir(),
        &sbxm::commands::task_resume::Options {
            repo_root: f.env.tmp.path().join("target-repo"),
            target: sbxm::commands::task_resume::Target::Spec(spec),
            rounds: None,
            should_fix: false,
            disk: Default::default(),
            identity: Some(common::cloud_task::identity()),
            forget_ambiguous: false,
        },
        &gone,
        &sbxm::github::fake::FakeGitHub::default(),
        &Gone,
        &sbxm::task::gates::FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );

    let out = String::from_utf8(out).unwrap();
    result.expect(&out);
    assert_eq!(shapes(&gone)[0], (worker_sandbox(&id), 8, "16g".to_owned()));
    let worker = task_record(&f, &id).worker.unwrap();
    assert_eq!((worker.cpus, worker.memory), (Some(8), Some("16g".into())));
}

struct Yes;

impl sbxm::confirm::Confirm for Yes {
    fn is_interactive(&self) -> bool {
        true
    }
    fn confirm(&self, _prompt: &str) -> anyhow::Result<bool> {
        Ok(true)
    }
}

/// `task run --spec --restart --cloud` with local resources that are no shape is refused before
/// the old task is discarded: its folder and its record stay as they were, and nothing is made.
#[test]
fn a_cloud_restart_with_no_shape_is_refused_before_the_old_task_is_discarded() {
    let f = fixture_with(NO_SHAPE_TOML);
    let (spec, id) = spec(&f);
    let root = f.env.tmp.path().join("cloud-fs");
    let failed = sbxm::backend::ExecOutput {
        stdout: String::new(),
        stderr: "boom\n".into(),
        exit_code: Some(1),
    };
    let backend = common::cloud_task::cloud_backend_worker(&root, &[&id], &[CLEAN], true, failed);
    assert!(
        task_run(&f, spec.clone(), Some(Location::Cloud), &backend)
            .result
            .is_err()
    );
    let record = sbxm::task::record::task_dir(&f.env.base_dir(), &id).join("task.json");
    let before = std::fs::read(&record).unwrap();
    set_global_resources(&f, 8, "4g");
    let again = cloud_backend(&root, &id, &[CLEAN]);

    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = sbxm::commands::task_run::run_spec(
        &f.env.config_dir(),
        &common::cloud_task::run_options(&f, spec, Some(Location::Cloud)),
        Some(&sbxm::commands::task_start::Restart {
            confirm: &Yes,
            yes: true,
        }),
        &again,
        &sbxm::github::fake::FakeGitHub::default(),
        &common::task_fixture::Probe,
        &sbxm::task::gates::FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );

    let message = format!("{:#}", result.unwrap_err());
    assert!(message.contains("set [sandbox] cloud_shape"), "{message}");
    assert_eq!(std::fs::read(&record).unwrap(), before);
    assert!(again.creates().is_empty(), "{:?}", again.log());
    assert!(again.removes().is_empty(), "{:?}", again.log());
}
