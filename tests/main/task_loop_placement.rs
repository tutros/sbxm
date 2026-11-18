//! Issue 648: `[resources] placement = "cloud-first"` sends the largest queued
//! issue to a free cloud slot and the first in the queue's order to a free local slot.

use crate::common;

// --- the setting ---

mod setting {
    use sbxm::config::{GlobalConfig, Placement};

    use super::common::Env;
    use super::set_placement;

    #[test]
    fn unset_means_local_first() {
        let env = Env::new();
        let config = GlobalConfig::load(&env.config_dir()).unwrap();
        assert_eq!(config.resources.placement, Placement::LocalFirst);
    }

    #[test]
    fn both_values_are_read() {
        for (text, placement) in [
            ("local-first", Placement::LocalFirst),
            ("cloud-first", Placement::CloudFirst),
        ] {
            let env = Env::new();
            set_placement(&env, text);
            let config = GlobalConfig::load(&env.config_dir()).unwrap();
            assert_eq!(config.resources.placement, placement);
        }
    }

    #[test]
    fn an_unknown_value_is_refused_naming_the_two_values() {
        let env = Env::new();
        set_placement(&env, "fastest");
        let err = format!("{:#}", GlobalConfig::load(&env.config_dir()).unwrap_err());
        assert!(err.contains("fastest"), "{err}");
        assert!(err.contains("local-first"), "{err}");
        assert!(err.contains("cloud-first"), "{err}");
        assert!(err.contains("config.toml"), "{err}");
    }

    #[test]
    fn the_setting_is_not_part_of_a_sandbox_config_hash() {
        let env = Env::new();
        let before = GlobalConfig::load(&env.config_dir()).unwrap();
        set_placement(&env, "cloud-first");
        let after = GlobalConfig::load(&env.config_dir()).unwrap();
        for harness in [
            sbxm::harness::Harness::Claude,
            sbxm::harness::Harness::Codex,
        ] {
            assert_eq!(
                before
                    .current_hash("demo", "default", harness, false)
                    .unwrap(),
                after
                    .current_hash("demo", "default", harness, false)
                    .unwrap()
            );
        }
    }
}

/// Sets `placement = "<value>"` in `[resources]` of `env`'s config.
fn set_placement(env: &common::Env, value: &str) {
    let path = env.config_dir().join("config.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("[resources]\n"), "{text}");
    let text = text.replacen(
        "[resources]\n",
        &format!("[resources]\nplacement = \"{value}\"\n"),
        1,
    );
    std::fs::write(&path, text).unwrap();
}

// --- routing ---

mod routing {
    use sbxm::github::fake::FakeGitHub;

    use super::common::task_fixture::{issue_text, ready_open_issue};
    use super::set_placement;
    use crate::task_loop_parallel::several::{Act, Fixture, Play, fixture, run_loop};

    const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

    fn never() -> bool {
        false
    }

    /// [`fixture`] with `placement = "cloud-first"`.
    fn cloud_first(cloud_slots: Option<u32>) -> Fixture {
        let f = fixture(cloud_slots);
        set_placement(&f.env, "cloud-first");
        f
    }

    /// GitHub with these issues open and labeled ready: each its size (`None`: unsized) and its
    /// `extra` labels.
    fn github(issues: &[(u32, Option<&str>, &[&str])]) -> FakeGitHub {
        let mut gh = FakeGitHub::default()
            .with_user("me")
            .with_default_branch("main")
            .with_branch_head("main", HEAD);
        let mut open = Vec::new();
        for (n, size, extra) in issues {
            gh = gh.with_issue_text(issue_text(*n));
            let size = size.map(|s| format!("size:{s}"));
            let mut labels = vec!["ready-for-worker"];
            labels.extend(size.as_deref());
            labels.extend_from_slice(extra);
            open.push(ready_open_issue(*n, &labels, "Why\n"));
        }
        gh.with_open_issues(open)
    }

    fn starts(play: &Play) -> Vec<String> {
        play.events()
            .into_iter()
            .filter(|e| e.starts_with("start"))
            .collect()
    }

    #[test]
    fn a_free_cloud_slot_takes_the_largest_issue_and_the_local_slot_the_smallest() {
        let f = cloud_first(Some(1));
        let gh = github(&[
            (10, Some("S"), &[]),
            (20, Some("L"), &[]),
            (30, Some("M"), &[]),
        ]);
        let play = Play::new(&f.base())
            .plays(20, &[(10, Act::Ready)])
            .plays(10, &[(3, Act::Ready)]);

        let run = run_loop(&f, &play, &gh, &never);

        run.result.unwrap();
        assert_eq!(
            starts(&play),
            [
                "start 20 run cloud",
                "start 10 run local",
                "start 30 run local"
            ],
            "{}",
            run.out
        );
        assert!(play.at("end 10 Ready") < play.at("start 30 run local"));
    }

    #[test]
    fn with_the_cloud_full_the_free_local_slot_takes_the_first_in_queue_order() {
        let f = cloud_first(Some(2));
        let gh = github(&[
            (1, Some("L"), &[]),
            (2, Some("L"), &[]),
            (10, Some("S"), &[]),
            (20, Some("L"), &[]),
        ]);
        let play = Play::new(&f.base())
            .plays(1, &[(10, Act::Ready)])
            .plays(2, &[(10, Act::Ready)])
            .plays(10, &[(20, Act::Ready)]);

        let run = run_loop(&f, &play, &gh, &never);

        run.result.unwrap();
        assert_eq!(
            starts(&play),
            [
                "start 1 run cloud",
                "start 2 run cloud",
                "start 10 run local",
                "start 20 run cloud"
            ],
            "{}",
            run.out
        );
        assert!(play.at("end 1 Ready") < play.at("start 20 run cloud"));
    }

    #[test]
    fn a_run_local_l_is_not_taken_for_the_cloud_and_the_next_largest_is() {
        let f = cloud_first(Some(1));
        let gh = github(&[
            (10, Some("S"), &[]),
            (20, Some("L"), &["run-local"]),
            (30, Some("M"), &[]),
        ]);
        let play = Play::new(&f.base())
            .plays(30, &[(10, Act::Ready)])
            .plays(10, &[(3, Act::Ready)]);

        let run = run_loop(&f, &play, &gh, &never);

        run.result.unwrap();
        assert_eq!(
            starts(&play),
            [
                "start 30 run cloud",
                "start 10 run local",
                "start 20 run local"
            ],
            "{}",
            run.out
        );
    }

    #[test]
    fn a_run_cloud_issue_never_starts_in_the_free_local_slot() {
        let f = cloud_first(Some(1));
        let gh = github(&[(1, Some("L"), &[]), (2, Some("S"), &["run-cloud"])]);
        let play = Play::new(&f.base()).plays(1, &[(5, Act::Ready)]);

        let run = run_loop(&f, &play, &gh, &never);

        run.result.unwrap();
        assert_eq!(
            starts(&play),
            ["start 1 run cloud", "start 2 run cloud"],
            "{}",
            run.out
        );
        assert!(play.at("end 1 Ready") < play.at("start 2 run cloud"));
    }

    #[test]
    fn with_cloud_slots_unset_cloud_first_is_ignored_one_task_at_a_time_locally() {
        let f = cloud_first(None);
        let gh = github(&[(10, Some("S"), &[]), (20, Some("L"), &[])]);
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
    fn of_two_l_issues_the_lower_number_goes_to_the_cloud_first() {
        let f = cloud_first(Some(1));
        let gh = github(&[(21, Some("L"), &[]), (20, Some("L"), &[])]);
        let play = Play::new(&f.base());

        let run = run_loop(&f, &play, &gh, &never);

        run.result.unwrap();
        assert_eq!(
            starts(&play),
            ["start 20 run cloud", "start 21 run local"],
            "{}",
            run.out
        );
    }

    #[test]
    fn an_s_issue_goes_to_the_cloud_and_an_unsized_one_is_skipped() {
        let f = cloud_first(Some(1));
        let gh = github(&[(5, None, &[]), (10, Some("S"), &[])]);
        let play = Play::new(&f.base());

        let run = run_loop(&f, &play, &gh, &never);

        run.result.unwrap();
        // An unsized issue fails the readiness rules (issue 725): it no longer runs last.
        assert_eq!(starts(&play), ["start 10 run cloud"], "{}", run.out);
        assert!(
            run.out.contains("skipped #5 Fix 5: no size label"),
            "{}",
            run.out
        );
    }

    #[test]
    fn local_first_is_routed_as_before_with_the_local_slot_taking_the_smallest() {
        let f = fixture(Some(1));
        set_placement(&f.env, "local-first");
        let gh = github(&[
            (10, Some("S"), &[]),
            (20, Some("L"), &[]),
            (30, Some("M"), &[]),
        ]);
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
}

// --- `sbxm status` ---

mod status {
    use sbxm::backend::FakeBackend;
    use sbxm::commands::status::{self, Env, LoopSource};
    use sbxm::config::{GlobalConfig, Placement};
    use sbxm::github::fake::FakeGitHub;
    use sbxm::task::record::ProcessProbe;

    use super::common;
    use super::set_placement;

    struct Gone;

    impl ProcessProbe for Gone {
        fn start_time(&self, _pid: u32) -> Option<u64> {
            None
        }
    }

    /// The Loop section, with `cloud_slots` set or not.
    fn loop_section(placement: Placement, cloud_slots: bool) -> String {
        let test = common::Env::new();
        let env = Env {
            now: Box::new(|| 1_790_000_000),
            sleep: Box::new(|_| {}),
            terminal: false,
            width: 120,
            iterations: None,
            monotonic: Box::new(|| std::time::Duration::ZERO),
            disk: sbxm::status::footer::Disk {
                free_space: |_| Ok(u64::MAX),
                same_drive: |_, _| true,
                state_dir: None,
                min_free_disk: 0,
            },
            slots: None,
            cloud_slots: cloud_slots.then(|| (2, common::slot::gate(&test, false))),
            placement,
        };
        let gh = FakeGitHub::default();
        let source = LoopSource {
            github: &gh,
            repo: Ok("o/r".into()),
        };
        let mut out = Vec::new();
        status::run_with(
            &test.base_dir(),
            &FakeBackend::default(),
            &Gone,
            &env,
            None,
            Some(&source),
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        let section = text.split("Loop:").nth(1).expect("a Loop section");
        section.split("\n\n").next().unwrap().to_owned()
    }

    #[test]
    fn cloud_first_is_shown_in_the_loop_section() {
        let section = loop_section(Placement::CloudFirst, true);
        assert!(
            section.contains("\n  placement: cloud-first\n"),
            "{section}"
        );
    }

    #[test]
    fn local_first_shows_no_placement() {
        let section = loop_section(Placement::LocalFirst, true);
        assert!(!section.contains("placement"), "{section}");
    }

    #[test]
    fn cloud_first_without_cloud_slots_shows_no_placement() {
        let section = loop_section(Placement::CloudFirst, false);
        assert!(!section.contains("placement"), "{section}");
    }

    #[test]
    fn the_real_env_reads_the_setting() {
        let test = common::Env::new();
        let unset = GlobalConfig::load(&test.config_dir()).unwrap();
        assert_eq!(Env::real(&unset).placement, Placement::LocalFirst);
        set_placement(&test, "cloud-first");
        let set = GlobalConfig::load(&test.config_dir()).unwrap();
        assert_eq!(Env::real(&set).placement, Placement::CloudFirst);
    }
}
