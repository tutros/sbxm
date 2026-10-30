//! Issue 330 (decision 270, point 3): `sbxm task` with `antigravity_auth = "login"`: an
//! Antigravity worker or reviewer needs the saved login and one working refresh, checked before
//! anything is written, and not the `google` secret.

mod common;

use std::fs;
use std::path::PathBuf;

use common::login::{Endpoint, write_login};
use common::task_fixture::{Fixture, ctx, fixture_with, issue_text, source};
use sbxm::backend::{FakeBackend, Location};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::pipeline;

const ANTIGRAVITY_WORKER: &str = "[sandbox]\nprofile = \"default\"\n\n[worker]\nharness = \
     \"antigravity\"\n\n[reviewer]\nharness = \"antigravity\"\n\n[gates]\nsandbox = [\"cargo \
     test\"]\n";

fn login_fixture() -> Fixture {
    let f = fixture_with(ANTIGRAVITY_WORKER);
    f.env.set_config("antigravity_auth = \"login\"");
    f
}

fn base_has_nothing_new(f: &Fixture) -> bool {
    let empty_or_absent =
        |dir: PathBuf| fs::read_dir(dir).map_or(true, |mut entries| entries.next().is_none());
    empty_or_absent(f.env.base_dir().join("tasks"))
        && empty_or_absent(f.env.base_dir().join(".sbxm").join("tasks"))
}

#[test]
fn a_login_mode_antigravity_worker_needs_no_google_secret() {
    let f = login_fixture();
    write_login(&f.env.config_dir());
    let (backend, github) = (FakeBackend::with_secrets(&[]), FakeGitHub::default());
    let endpoint = Endpoint::fresh("new", 3599);
    let source = source(&f);
    let host = endpoint.host();
    let mut ctx = ctx(&f, &source, &backend, &github);
    ctx.auth = &host;
    pipeline::prepare(&ctx, &issue_text(41)).unwrap();
    assert_eq!(endpoint.calls(), 1);
}

#[test]
fn a_login_mode_antigravity_worker_without_a_login_is_refused_before_anything_is_written() {
    let f = login_fixture();
    let (backend, github) = (
        FakeBackend::with_secrets(&["google"]),
        FakeGitHub::default(),
    );
    let endpoint = Endpoint::fresh("new", 3599);
    let source = source(&f);
    let host = endpoint.host();
    let mut ctx = ctx(&f, &source, &backend, &github);
    ctx.auth = &host;
    let err = format!(
        "{:#}",
        pipeline::prepare(&ctx, &issue_text(41)).unwrap_err()
    );
    assert!(err.contains("sbxm auth antigravity"), "{err}");
    assert_eq!(err.lines().count(), 1, "{err}");
    assert!(base_has_nothing_new(&f));
    assert!(backend.log().is_empty(), "{:?}", backend.log());
}

#[test]
fn a_login_mode_antigravity_reviewer_needs_the_login_not_the_google_secret() {
    let f = login_fixture();
    let backend = FakeBackend::with_secrets(&[]);
    let refused = Endpoint::answering(401, "{}");
    let err = pipeline::check_reviewer_inputs(
        &f.env.config_dir(),
        &backend,
        &f.config,
        Location::Local,
        &refused.host(),
    )
    .unwrap_err();
    assert!(
        format!("{err:#}").contains("sbxm auth antigravity"),
        "{err:#}"
    );

    write_login(&f.env.config_dir());
    let endpoint = Endpoint::fresh("new", 3599);
    pipeline::check_reviewer_inputs(
        &f.env.config_dir(),
        &backend,
        &f.config,
        Location::Local,
        &endpoint.host(),
    )
    .unwrap();
    pipeline::check_worker_inputs(
        &f.env.config_dir(),
        &backend,
        &f.config,
        Location::Local,
        &endpoint.host(),
    )
    .unwrap();
}

#[test]
fn api_key_mode_still_needs_the_google_secret() {
    let f = fixture_with(ANTIGRAVITY_WORKER);
    let backend = FakeBackend::with_secrets(&[]);
    let endpoint = Endpoint::fresh("new", 3599);
    let err = pipeline::check_worker_inputs(
        &f.env.config_dir(),
        &backend,
        &f.config,
        Location::Local,
        &endpoint.host(),
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("'google'"), "{err:#}");
    assert_eq!(endpoint.calls(), 0);
}

/// The process that made a task is gone, so `task rm` may remove it.
struct Gone;

impl sbxm::task::record::ProcessProbe for Gone {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

/// The positions in `log` of the entry equal to `entry`.
fn at(log: &[String], entry: &str) -> usize {
    log.iter()
        .position(|l| l == entry)
        .unwrap_or_else(|| panic!("no {entry:?} in {log:#?}"))
}

const WORKER_SANDBOX: &str = "sbxm-task-issue-41-antigravity";

fn worker_sandbox(backend: &FakeBackend) -> String {
    backend
        .log()
        .iter()
        .find_map(|l| l.strip_prefix("create "))
        .expect("a create")
        .to_owned()
}

#[test]
fn a_login_mode_worker_gets_its_secret_before_its_sandbox_and_its_token_file_after() {
    let f = login_fixture();
    write_login(&f.env.config_dir());
    let (backend, github) = (FakeBackend::with_secrets(&[]), FakeGitHub::default());
    let endpoint = Endpoint::fresh("new", 3599);
    let source = source(&f);
    let host = endpoint.host();
    let mut ctx = ctx(&f, &source, &backend, &github);
    ctx.auth = &host;
    pipeline::prepare(&ctx, &issue_text(41)).unwrap();

    let sandbox = worker_sandbox(&backend);
    assert_eq!(sandbox, WORKER_SANDBOX);
    let log = backend.log();
    let placeholder = sbxm::auth::placeholder(&sandbox);
    let set = at(&log, &format!("secret-set {sandbox} {placeholder}"));
    let create = at(&log, &format!("create {sandbox}"));
    let exec = at(&log, &format!("exec {sandbox}"));
    assert!(set < create && create < exec, "{log:#?}");
    let (_, spec) = &backend.execs()[0];
    assert!(spec.argv.join(" ").contains("antigravity-oauth-token"));
    assert_eq!(
        backend.custom_secret_sets()[0].source,
        sbxm::backend::SecretSource::CommandLine(
            sbxm::auth::helper_command(&common::login::helper_path()).unwrap()
        )
    );
}

#[test]
fn a_failed_login_mode_start_removes_the_sandbox_then_its_secret() {
    let f = login_fixture();
    write_login(&f.env.config_dir());
    let (backend, github) = (FakeBackend::failing_create(), FakeGitHub::default());
    let endpoint = Endpoint::fresh("new", 3599);
    let source = source(&f);
    let host = endpoint.host();
    let mut ctx = ctx(&f, &source, &backend, &github);
    ctx.auth = &host;
    pipeline::prepare(&ctx, &issue_text(41)).unwrap_err();

    let log = backend.log();
    let placeholder = sbxm::auth::placeholder(WORKER_SANDBOX);
    let removed = at(&log, &format!("rm {WORKER_SANDBOX}"));
    let unset = at(&log, &format!("secret-rm {WORKER_SANDBOX} {placeholder}"));
    assert!(removed < unset, "{log:#?}");
    assert!(base_has_nothing_new(&f));
}

#[test]
fn task_rm_removes_an_antigravity_sandboxs_secret_after_it_whatever_the_mode() {
    for login in [true, false] {
        let f = fixture_with(ANTIGRAVITY_WORKER);
        if login {
            f.env.set_config("antigravity_auth = \"login\"");
            write_login(&f.env.config_dir());
        }
        let (backend, github) = (
            FakeBackend::with_secrets(&["google"]),
            FakeGitHub::default(),
        );
        let endpoint = Endpoint::fresh("new", 3599);
        let source = source(&f);
        let host = endpoint.host();
        let mut ctx = ctx(&f, &source, &backend, &github);
        ctx.auth = &host;
        pipeline::prepare(&ctx, &issue_text(41)).unwrap();

        let plan = sbxm::task::finish::plan_removal(&f.env.base_dir(), "issue-41", &Gone).unwrap();
        let report = sbxm::task::finish::remove(&plan, &backend);
        assert!(report.is_clean(), "{report:?}");
        let log = backend.log();
        let placeholder = sbxm::auth::placeholder(WORKER_SANDBOX);
        let removed = at(&log, &format!("rm {WORKER_SANDBOX}"));
        let unset = at(&log, &format!("secret-rm {WORKER_SANDBOX} {placeholder}"));
        assert!(removed < unset, "{log:#?}");
    }
}

#[test]
fn task_rm_of_a_claude_task_touches_no_secret() {
    let f = common::task_fixture::fixture();
    let (backend, github) = (
        FakeBackend::with_secrets(&["anthropic"]),
        FakeGitHub::default(),
    );
    let source = source(&f);
    pipeline::prepare(&ctx(&f, &source, &backend, &github), &issue_text(41)).unwrap();
    let plan = sbxm::task::finish::plan_removal(&f.env.base_dir(), "issue-41", &Gone).unwrap();
    sbxm::task::finish::remove(&plan, &backend);
    assert!(
        !backend.log().iter().any(|l| l.starts_with("secret-")),
        "{:#?}",
        backend.log()
    );
}

const AGY_DONE: &str = include_str!("../src/fixtures/antigravity-stream-json-pong.jsonl");

#[test]
fn a_login_mode_reviewer_gets_its_secret_and_token_file_and_loses_the_secret_after_its_sandbox() {
    let f = fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[reviewer]\nharness = \"antigravity\"\n\n[gates]\n\
         sandbox = [\"cargo test\"]\n",
    );
    f.env.set_config("antigravity_auth = \"login\"");
    write_login(&f.env.config_dir());
    let worker = common::task_fixture::play_tasks(
        &f.env.base_dir(),
        "main",
        common::task_fixture::Play {
            commits: vec!["a.txt".into()],
            result_md: Some(b"done\n".to_vec()),
            bundle_bytes: None,
        },
    );
    let reviewer = common::task_fixture::play_reviews(
        &f.env.base_dir(),
        "issue-41",
        vec!["Must-fix findings: 0\n\nNothing found.\n".into()],
    );
    let backend = FakeBackend::with_secrets(&["anthropic"])
        .with_exec_output_matching(
            "claude",
            common::task_fixture::ok(common::task_fixture::CLAUDE_DONE),
        )
        .with_exec_output_matching("agy", common::task_fixture::ok(AGY_DONE))
        .with_exec_hook(move |sandbox, spec| {
            worker(sandbox, spec);
            reviewer(sandbox, spec);
        });
    let mut prepared = common::task_fixture::worked_task(&f, &backend);
    let github = FakeGitHub::default();
    let source = source(&f);
    let endpoint = Endpoint::fresh("new", 3599);
    let host = endpoint.host();
    let mut ctx = ctx(&f, &source, &backend, &github);
    ctx.auth = &host;
    let gated = pipeline::run_gates(
        &ctx.env(),
        &mut prepared,
        "after-worker",
        pipeline::Tiers::ALL,
    )
    .unwrap();
    assert!(gated.passed);
    pipeline::run_reviewer(&ctx.env(), &mut prepared, 1).unwrap();

    let sandbox = "sbxm-task-issue-41-review-antigravity";
    let log = backend.log();
    let placeholder = sbxm::auth::placeholder(sandbox);
    let set = at(&log, &format!("secret-set {sandbox} {placeholder}"));
    let create = at(&log, &format!("create {sandbox}"));
    let removed = at(&log, &format!("rm {sandbox}"));
    let unset = at(&log, &format!("secret-rm {sandbox} {placeholder}"));
    assert!(
        set < create && create < removed && removed < unset,
        "{log:#?}"
    );
    let execs: Vec<_> = backend
        .execs()
        .into_iter()
        .filter(|(s, _)| s == sandbox)
        .collect();
    assert!(
        execs[0]
            .1
            .argv
            .join(" ")
            .contains("antigravity-oauth-token")
    );
    // The Claude worker's sandbox has none.
    assert!(
        !log.iter()
            .any(|l| l.starts_with("secret-") && l.contains("issue-41-claude")),
        "{log:#?}"
    );
}

#[test]
fn a_task_refused_by_a_later_check_leaves_no_token_cache() {
    let f = login_fixture();
    write_login(&f.env.config_dir());
    let backend = FakeBackend::with_secrets(&[]);
    backend
        .secret_services_failure()
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let endpoint = Endpoint::fresh("new", 3599);
    pipeline::check_worker_inputs(
        &f.env.config_dir(),
        &backend,
        &f.config,
        Location::Local,
        &endpoint.host(),
    )
    .unwrap_err();
    assert_eq!(endpoint.calls(), 1);
    assert!(!sbxm::auth::cache_path(&f.env.config_dir()).exists());
}

#[test]
fn a_failed_start_whose_secret_stays_names_the_removal_and_keeps_the_task() {
    let f = login_fixture();
    write_login(&f.env.config_dir());
    let backend = FakeBackend::failing_create().failing_remove_custom_secret();
    let github = FakeGitHub::default();
    let endpoint = Endpoint::fresh("new", 3599);
    let source = source(&f);
    let host = endpoint.host();
    let mut ctx = ctx(&f, &source, &backend, &github);
    ctx.auth = &host;
    let err = format!(
        "{:#}",
        pipeline::prepare(&ctx, &issue_text(41)).unwrap_err()
    );
    let placeholder = sbxm::auth::placeholder(WORKER_SANDBOX);
    assert!(
        err.contains(&format!(
            "sbx secret rm --placeholder {placeholder} --sandbox {WORKER_SANDBOX} -f"
        )),
        "{err}"
    );
    assert!(err.contains("cannot create sandbox"), "{err}");
    // The task stays, so `sbxm task rm` can remove the secret later.
    let plan = sbxm::task::finish::plan_removal(&f.env.base_dir(), "issue-41", &Gone).unwrap();
    assert_eq!(plan.antigravity, vec![WORKER_SANDBOX.to_owned()]);
}

/// A worked Claude task whose reviewer is a login-mode Antigravity one, on `backend`'s scripts.
fn reviewer_fixture() -> Fixture {
    let f = fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[reviewer]\nharness = \"antigravity\"\n\n[gates]\n\
         sandbox = [\"cargo test\"]\n",
    );
    f.env.set_config("antigravity_auth = \"login\"");
    write_login(&f.env.config_dir());
    f
}

fn reviewer_backend(f: &Fixture) -> FakeBackend {
    let worker = common::task_fixture::play_tasks(
        &f.env.base_dir(),
        "main",
        common::task_fixture::Play {
            commits: vec!["a.txt".into()],
            result_md: Some(b"done\n".to_vec()),
            bundle_bytes: None,
        },
    );
    let reviewer = common::task_fixture::play_reviews(
        &f.env.base_dir(),
        "issue-41",
        vec!["Must-fix findings: 0\n\nNothing found.\n".into()],
    );
    FakeBackend::with_secrets(&["anthropic"])
        .with_exec_output_matching(
            "claude",
            common::task_fixture::ok(common::task_fixture::CLAUDE_DONE),
        )
        .with_exec_output_matching("agy", common::task_fixture::ok(AGY_DONE))
        .with_exec_hook(move |sandbox, spec| {
            worker(sandbox, spec);
            reviewer(sandbox, spec);
        })
        .failing_remove_custom_secret()
}

fn gate(ctx: &pipeline::Ctx, prepared: &mut pipeline::Prepared) {
    let gated =
        pipeline::run_gates(&ctx.env(), prepared, "after-worker", pipeline::Tiers::ALL).unwrap();
    assert!(gated.passed);
}

const REVIEW_SANDBOX: &str = "sbxm-task-issue-41-review-antigravity";

#[test]
fn a_reviewer_secret_that_stays_is_noted_with_its_removal() {
    let f = reviewer_fixture();
    let backend = reviewer_backend(&f);
    let mut prepared = common::task_fixture::worked_task(&f, &backend);
    let github = FakeGitHub::default();
    let source = source(&f);
    let endpoint = Endpoint::fresh("new", 3599);
    let host = endpoint.host();
    let mut ctx = ctx(&f, &source, &backend, &github);
    ctx.auth = &host;
    gate(&ctx, &mut prepared);
    pipeline::run_reviewer(&ctx.env(), &mut prepared, 1).unwrap();

    let removal = format!(
        "sbx secret rm --placeholder {} --sandbox {REVIEW_SANDBOX} -f",
        sbxm::auth::placeholder(REVIEW_SANDBOX)
    );
    assert!(
        prepared.record.notes.iter().any(|n| n.contains(&removal)),
        "{:#?}",
        prepared.record.notes
    );
}

#[test]
fn a_leftover_reviewer_whose_secret_stays_stops_the_round_before_a_new_sandbox() {
    let f = reviewer_fixture();
    let backend = reviewer_backend(&f);
    let mut prepared = common::task_fixture::worked_task(&f, &backend);
    fs::create_dir_all(prepared.workspace.with_file_name("issue-41-review")).unwrap();
    let github = FakeGitHub::default();
    let source = source(&f);
    let endpoint = Endpoint::fresh("new", 3599);
    let host = endpoint.host();
    let mut ctx = ctx(&f, &source, &backend, &github);
    ctx.auth = &host;
    gate(&ctx, &mut prepared);
    let err = format!(
        "{:#}",
        pipeline::run_reviewer(&ctx.env(), &mut prepared, 1).unwrap_err()
    );
    assert!(err.contains("sbx secret rm --placeholder"), "{err}");
    assert!(
        !backend.log().contains(&format!("create {REVIEW_SANDBOX}")),
        "{:#?}",
        backend.log()
    );
}
