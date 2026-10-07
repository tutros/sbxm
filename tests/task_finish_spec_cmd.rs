//! Issue 142 (decision 174(e)): `sbxm task finish --spec <FILE>` as a command: always refused,
//! with a clear message, since the sink isn't built yet.

mod common;

use std::fs;
use std::path::PathBuf;

use common::task_fixture::{Fixture, Probe, backend, ctx, fixture, source};
use sbxm::commands::task_finish::{Options, Target, run};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::pipeline;

fn spec_file(f: &Fixture, text: &str) -> PathBuf {
    let path = f.env.tmp.path().join("idea.md");
    fs::write(&path, text).unwrap();
    path
}

#[test]
fn finish_is_refused_for_a_prepared_spec_task() {
    let f = fixture();
    let spec = spec_file(&f, "Build a thing.\n");
    let b = backend();
    let github = FakeGitHub::default();
    let source = source(&f);
    pipeline::prepare_spec(&ctx(&f, &source, &b, &github), &spec).unwrap();

    let mut out = Vec::new();
    let result = run(
        &f.env.config_dir(),
        &Options {
            target: Target::Spec(spec),
        },
        &FakeGitHub::default(),
        &Probe,
        &mut out,
    );

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("spec-idea-") && message.contains("sink"),
        "{message}"
    );
}

#[test]
fn finish_names_the_missing_task_when_no_task_was_started() {
    let f = fixture();
    let spec = spec_file(&f, "Build a thing.\n");
    let mut out = Vec::new();

    let result = run(
        &f.env.config_dir(),
        &Options {
            target: Target::Spec(spec),
        },
        &FakeGitHub::default(),
        &Probe,
        &mut out,
    );

    let message = format!("{:#}", result.unwrap_err());
    assert!(message.contains("no task"), "{message}");
}
