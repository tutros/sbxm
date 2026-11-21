//! Issue 735: `[resources] placement` defaults to `cloud-first` (decision 326, adopted at the
//! 2026-11-21 retro); `local-first` stays as the off switch, and with `cloud_slots` unset the
//! setting isn't read.

use sbxm::config::{GlobalConfig, Placement};
use sbxm::github::fake::FakeGitHub;

use crate::common::Env;
use crate::common::cloud_slot::set_placement;
use crate::common::task_fixture::{issue_text, ready_open_issue};
use crate::task_loop_parallel::several::{Act, Fixture, Play, fixture, run_loop};

const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

fn never() -> bool {
    false
}

/// GitHub with these issues open and labeled ready, each with its size.
fn github(issues: &[(u32, &str)]) -> FakeGitHub {
    let mut gh = FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_branch_head("main", HEAD);
    let mut open = Vec::new();
    for (n, size) in issues {
        gh = gh.with_issue_text(issue_text(*n));
        let size = format!("size:{size}");
        open.push(ready_open_issue(*n, &["ready-for-worker", &size], "Why\n"));
    }
    gh.with_open_issues(open)
}

fn starts(play: &Play) -> Vec<String> {
    play.events()
        .into_iter()
        .filter(|e| e.starts_with("start"))
        .collect()
}

/// An S, an L and an M issue with one cloud slot: issue 10 (S) holds its slot for 3 steps,
/// the one in the cloud for 10, so the third issue starts locally after issue 10 ends.
fn run_s_l_m(f: &Fixture, cloud_issue: u32) -> (Play<'_>, String) {
    let gh = github(&[(10, "S"), (20, "L"), (30, "M")]);
    let play = Play::new(&f.base())
        .plays(cloud_issue, &[(10, Act::Ready)])
        .plays(10, &[(3, Act::Ready)]);
    let run = run_loop(f, &play, &gh, &never);
    run.result.unwrap();
    (play, run.out)
}

#[test]
fn task_loop_placement_default_unset_reads_as_cloud_first() {
    let env = Env::new();
    let config = GlobalConfig::load(&env.config_dir()).unwrap();
    assert_eq!(config.resources.placement, Placement::CloudFirst);
}

#[test]
fn task_loop_placement_default_unset_routes_the_largest_issue_to_the_cloud() {
    let f = fixture(Some(1));

    let (play, out) = run_s_l_m(&f, 20);

    assert_eq!(
        starts(&play),
        [
            "start 20 run cloud",
            "start 10 run local",
            "start 30 run local"
        ],
        "{out}"
    );
}

#[test]
fn task_loop_placement_default_local_first_is_the_off_switch() {
    let f = fixture(Some(1));
    set_placement(&f.env, "local-first");
    let gh = github(&[(10, "S"), (20, "L"), (30, "M")]);
    let play = Play::new(&f.base())
        .plays(10, &[(3, Act::Ready)])
        .plays(30, &[(10, Act::Ready)]);

    let run = run_loop(&f, &play, &gh, &never);

    run.result.unwrap();
    assert_eq!(
        starts(&play),
        [
            "start 10 run local",
            "start 30 run cloud",
            "start 20 run local"
        ],
        "{}",
        run.out
    );
}

#[test]
fn task_loop_placement_default_cloud_first_set_is_unchanged() {
    let f = fixture(Some(1));
    set_placement(&f.env, "cloud-first");

    let (play, out) = run_s_l_m(&f, 20);

    assert_eq!(
        starts(&play),
        [
            "start 20 run cloud",
            "start 10 run local",
            "start 30 run local"
        ],
        "{out}"
    );
}

#[test]
fn task_loop_placement_default_unset_without_cloud_slots_runs_every_task_locally() {
    let f = fixture(None);
    let gh = github(&[(10, "S"), (20, "L")]);
    let play = Play::new(&f.base());

    let run = run_loop(&f, &play, &gh, &never);

    run.result.unwrap();
    assert_eq!(
        play.events(),
        [
            "start 10 run local",
            "end 10 Ready",
            "finish 10",
            "start 20 run local",
            "end 20 Ready",
            "finish 20",
        ],
        "{}",
        run.out
    );
}

#[test]
fn task_loop_placement_default_status_reads_cloud_first_when_unset() {
    let env = Env::new();
    let config = GlobalConfig::load(&env.config_dir()).unwrap();
    assert_eq!(
        sbxm::commands::status::Env::real(&config).placement,
        Placement::CloudFirst
    );
}
