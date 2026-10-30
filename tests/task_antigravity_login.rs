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
