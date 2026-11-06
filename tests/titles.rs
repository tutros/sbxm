//! Issue 246 (decision 295): sbxm shows a short title with every task, issue and PR it names,
//! through one shared formatter (`sbxm::title`), from the title stored in `task.json` (no GitHub
//! call for it). Its own file (decision 292).

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::task_fixture::{Fixture, backend, ctx, fixture, source};
use sbxm::github::fake::FakeGitHub;
use sbxm::status::tasks::{self, TaskRow};
use sbxm::task::record::{self, Kind, NewTask, Process, Record};
use sbxm::task::{fixes, pipeline};
use sbxm::title;

const T0: u64 = 1_790_000_000;

fn task(kind: Kind, number: u32, title: &str) -> Record {
    Record::new(
        &NewTask {
            kind,
            number,
            repo: "o/r",
            title,
            base: "main",
            branch: "b",
            config_hash: "h",
            id: None,
        },
        T0,
        Process::new(1, T0),
    )
}

fn save(base: &Path, record: &Record) {
    record::write(&record::task_dir(base, &record.id), record).unwrap();
}

// ---- the formatter ---------------------------------------------------------

#[test]
fn the_formatter_puts_the_title_after_the_number() {
    assert_eq!(title::titled("#41", "Add titles"), "#41: Add titles");
    assert_eq!(title::named("PR #9", "Add titles"), "PR #9 (Add titles)");
}

#[test]
fn an_empty_title_shows_the_number_alone() {
    for empty in ["", "   ", "\n"] {
        assert_eq!(title::titled("#41", empty), "#41");
        assert_eq!(title::named("PR #9", empty), "PR #9");
    }
}

#[test]
fn a_very_long_title_is_shown_in_full_on_one_line() {
    let long = "word ".repeat(100);
    let shown = title::titled("#41", &format!("{long}\nsecond line"));
    assert_eq!(shown, format!("#41: {long} second line"));
    assert_eq!(shown.lines().count(), 1);
}

// ---- spec titles -----------------------------------------------------------

#[test]
fn a_spec_title_is_its_first_heading() {
    let text = "Some intro.\n\n## A section\n\n# Make status show titles\n\n# Later heading\n";
    assert_eq!(fixes::title(text, "idea.md"), "Make status show titles");
}

#[test]
fn a_spec_with_no_heading_is_titled_by_its_file_name() {
    for text in [
        "Build a thing.\n",
        "#hashtag is not a heading\n",
        "# \n",
        "",
    ] {
        assert_eq!(fixes::title(text, "idea.md"), "idea.md", "{text:?}");
    }
}

fn spec_file(f: &Fixture, name: &str, text: &str) -> PathBuf {
    let path = f.env.tmp.path().join(name);
    fs::write(&path, text).unwrap();
    path
}

#[test]
fn a_prepared_spec_task_records_its_first_heading_as_the_title() {
    let f = fixture();
    let (backend, github) = (backend(), FakeGitHub::default());
    let source = source(&f);
    let spec = spec_file(&f, "idea.md", "# Build a thing\n\nDetails.\n");

    let prepared = pipeline::prepare_spec(&ctx(&f, &source, &backend, &github), &spec).unwrap();

    let saved = record::read(&prepared.meta.join("task.json")).unwrap();
    assert_eq!(saved.title, "Build a thing");
}

#[test]
fn a_prepared_spec_task_without_a_heading_records_its_file_name() {
    let f = fixture();
    let (backend, github) = (backend(), FakeGitHub::default());
    let source = source(&f);
    let spec = spec_file(&f, "idea.md", "Build a thing.\n");

    let prepared = pipeline::prepare_spec(&ctx(&f, &source, &backend, &github), &spec).unwrap();

    assert_eq!(prepared.record.title, "idea.md");
}

// ---- the record --------------------------------------------------------------

#[test]
fn an_old_task_json_without_a_title_still_loads_and_shows_the_number_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let record = task(Kind::Issue, 41, "gone");
    save(base, &record);
    let path = record::task_dir(base, "issue-41").join("task.json");
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    value.as_object_mut().unwrap().remove("title");
    fs::write(&path, serde_json::to_string(&value).unwrap()).unwrap();

    let loaded = record::read(&path).unwrap();

    assert_eq!(loaded.title, "");
    assert_eq!(
        title::titled(&title::reference(&loaded), &loaded.title),
        "#41"
    );
}

#[test]
fn a_pr_is_titled_from_the_task_record_that_opened_it() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let mut opened = task(Kind::Issue, 41, "Add titles");
    opened.pr = Some("https://github.com/o/r/pull/9".to_owned());
    save(base, &opened);
    save(base, &task(Kind::Pr, 12, "Review me"));

    assert_eq!(title::pr_named(base, 9), "PR #9 (Add titles)");
    assert_eq!(title::pr_named(base, 12), "PR #12 (Review me)");
    // Its task record is gone (`task rm --merged`): the number alone.
    assert_eq!(title::pr_named(base, 13), "PR #13");
}

// ---- `sbxm status`'s title column ------------------------------------------

/// A row whose fixed columns (everything but the title and the log) are 54 columns wide with
/// their gaps: `issue-1`, `working`, `running` and four dashes under headers of 4, 5, 8 and 2.
fn row(title: &str) -> TaskRow {
    TaskRow {
        id: "issue-1".to_owned(),
        title: title.to_owned(),
        stage: "working",
        status: "running".to_owned(),
        stage_entered_at: None,
        commits_ahead: None,
        must_fix: None,
        pr: None,
        log_line: Some("[working] busy".to_owned()),
    }
}

/// The fixed columns, the title's own 2-column gap and the log's 1 column for its `…`.
const AROUND_TITLE: usize = 54 + 2 + 1;

#[test]
fn status_shows_a_title_column_after_the_id() {
    let text = tasks::render(&[row("Add titles")], 0, None);

    insta::assert_snapshot!(text, @r"
    ID       TITLE       STAGE    STATUS   TIME  AHEAD  FINDINGS  PR  LOG
    issue-1  Add titles  working  running  -     -      -         -   [working] busy
    ");
}

#[test]
fn status_off_a_terminal_never_cuts_a_long_title() {
    let long = "t".repeat(300);
    let text = tasks::render(&[row(&long)], 0, None);
    assert!(text.contains(&long), "{text}");
}

#[test]
fn status_on_a_terminal_shows_a_title_exactly_as_wide_as_the_space_left() {
    let title = "t".repeat(20);
    let text = tasks::render(&[row(&title)], 0, Some(AROUND_TITLE + 20));
    let line = text.lines().nth(1).unwrap();
    assert!(
        line.contains(&format!("issue-1  {title}  working")),
        "{text}"
    );
    assert!(line.ends_with('…'), "the log keeps its one column: {text}");
}

#[test]
fn status_on_a_terminal_cuts_a_title_one_column_too_wide() {
    let title = "t".repeat(20);
    let text = tasks::render(&[row(&title)], 0, Some(AROUND_TITLE + 19));
    let line = text.lines().nth(1).unwrap();
    assert!(
        line.contains(&format!("issue-1  {}…  working", "t".repeat(18))),
        "{text}"
    );
}

#[test]
fn status_on_a_terminal_shows_the_title_with_ten_columns_left() {
    let text = tasks::render(&[row(&"t".repeat(20))], 0, Some(AROUND_TITLE + 10));
    assert!(text.starts_with("ID       TITLE"), "{text}");
    assert!(
        text.contains(&format!("issue-1  {}…  working", "t".repeat(9))),
        "{text}"
    );
}

#[test]
fn status_on_a_terminal_drops_the_title_with_nine_columns_left() {
    let text = tasks::render(&[row(&"t".repeat(20))], 0, Some(AROUND_TITLE + 9));
    assert!(!text.contains("TITLE"), "{text}");
    assert!(!text.contains("ttt"), "{text}");
    assert!(text.contains("issue-1  working"), "{text}");
}

#[test]
fn status_without_any_title_has_no_title_column() {
    let text = tasks::render(&[row("")], 0, None);
    assert!(!text.contains("TITLE"), "{text}");
}

#[test]
fn status_rows_read_the_title_from_task_json() {
    struct Dead;
    impl record::ProcessProbe for Dead {
        fn start_time(&self, _pid: u32) -> Option<u64> {
            None
        }
    }
    let tmp = tempfile::tempdir().unwrap();
    save(tmp.path(), &task(Kind::Issue, 41, "Add titles"));

    let rows = tasks::rows(tmp.path(), &Dead).unwrap();

    assert_eq!(rows[0].title, "Add titles");
}

// ---- a renamed PR (answer 4) -------------------------------------------------

mod renamed {
    use super::*;
    use common::task_fixture::{
        CODEX_DONE, add_pr_head, ctx_with_host, fixture_with, issue_text, ok, play_reviews,
    };
    use sbxm::backend::FakeBackend;
    use sbxm::github::{PrInfo, PrState};
    use sbxm::task::gates::FakeHostRunner;
    use sbxm::task::pipeline::Prepared;

    const CLEAN: &str = "Must-fix findings: 0\n\nNothing found.\n";

    fn pr(title: &str) -> PrInfo {
        PrInfo {
            number: 7,
            head_ref: "feature-x".into(),
            head_oid: "0".repeat(40),
            is_cross_repository: false,
            head_repo: String::new(),
            state: PrState::Open,
            title: title.into(),
            body: "Adds x.\n\nFixes #4".into(),
            closing_issues: vec![4],
            base_moved: false,
        }
    }

    fn github(title: &str) -> FakeGitHub {
        FakeGitHub::default()
            .with_pr(pr(title))
            .with_issue_text(issue_text(4))
    }

    fn reviewed_once(f: &Fixture, backend: &FakeBackend) -> Prepared {
        add_pr_head(f, 7);
        let source = source(f);
        let github = github("Add the x feature");
        let mut prepared =
            pipeline::prepare_pr(&ctx(f, &source, backend, &github), &pr("Add the x feature"))
                .unwrap();
        review(f, backend, &github, &mut prepared).unwrap();
        prepared
    }

    fn review(
        f: &Fixture,
        backend: &FakeBackend,
        github: &FakeGitHub,
        prepared: &mut Prepared,
    ) -> anyhow::Result<pipeline::PrReview> {
        let source = source(f);
        let host = FakeHostRunner::default();
        let context = ctx_with_host(f, &source, backend, github, &host);
        pipeline::review_pr(&context.env(), github, prepared, true)
    }

    fn setup() -> (Fixture, FakeBackend) {
        let f = fixture_with(
            "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
             [reviewer]\nharness = \"codex\"\n",
        );
        let backend = FakeBackend::with_secrets(&["anthropic", "openai"])
            .with_exec_output_matching("codex", ok(CODEX_DONE))
            .with_exec_hook(play_reviews(
                &f.env.base_dir(),
                "pr-7",
                vec![CLEAN.to_owned(), CLEAN.to_owned()],
            ));
        (f, backend)
    }

    fn saved(f: &Fixture) -> Record {
        record::read(&record::task_dir(&f.env.base_dir(), "pr-7").join("task.json")).unwrap()
    }

    #[test]
    fn a_review_again_takes_the_renamed_prs_title_from_its_own_read() {
        let (f, backend) = setup();
        let mut prepared = reviewed_once(&f, &backend);
        assert_eq!(saved(&f).title, "Add the x feature");

        review(&f, &backend, &github("Add the y feature"), &mut prepared).unwrap();

        assert_eq!(saved(&f).title, "Add the y feature");
    }

    #[test]
    fn a_failed_read_keeps_the_stored_title() {
        let (f, backend) = setup();
        let mut prepared = reviewed_once(&f, &backend);

        review(&f, &backend, &FakeGitHub::default(), &mut prepared).unwrap_err();

        assert_eq!(saved(&f).title, "Add the x feature");
    }
}

// ---- `next:` hints and `task logs` -------------------------------------------

#[test]
fn a_next_hint_ends_with_the_tasks_title_as_a_shell_comment() {
    assert_eq!(
        title::hint_comment(&task(Kind::Issue, 41, "Add titles")),
        "  # #41: Add titles"
    );
    assert_eq!(
        title::hint_comment(&task(Kind::Pr, 9, "Review me")),
        "  # #9: Review me"
    );
    assert_eq!(title::hint_comment(&task(Kind::Issue, 41, "")), "");
}

#[test]
fn a_spec_tasks_hint_names_its_id_and_heading() {
    let mut spec = task(Kind::Spec, 0, "Build a thing");
    spec.id = "spec-idea-1fa7c2".to_owned();
    assert_eq!(
        title::hint_comment(&spec),
        "  # spec-idea-1fa7c2: Build a thing"
    );
}

fn logs(base_env: &common::Env, number: u32) -> String {
    use sbxm::commands::task_logs::{self, Options, Target};
    let mut out = Vec::new();
    task_logs::run(
        &base_env.config_dir(),
        &Options {
            target: Target::Issue(number),
            follow: false,
        },
        &task_logs::Env::real(),
        &mut out,
    )
    .unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn task_logs_names_the_task_with_its_title_first() {
    let env = common::Env::new();
    let record = task(Kind::Issue, 41, "Add titles");
    save(&env.base_dir(), &record);
    fs::write(
        record::task_dir(&env.base_dir(), "issue-41").join("run.log"),
        "# header\nline\n",
    )
    .unwrap();

    assert_eq!(
        logs(&env, 41),
        "# task issue-41: Add titles\n# header\nline\n"
    );
}

#[test]
fn task_logs_of_an_old_record_without_a_title_names_the_task_alone() {
    let env = common::Env::new();
    save(&env.base_dir(), &task(Kind::Issue, 41, ""));
    fs::write(
        record::task_dir(&env.base_dir(), "issue-41").join("run.log"),
        "line\n",
    )
    .unwrap();

    assert_eq!(logs(&env, 41), "# task issue-41\nline\n");
}

// ---- `sbxm status`'s Sandboxes and Loop sections (answer 5) ------------------

#[test]
fn a_tasks_sandbox_names_its_owner_with_the_title() {
    use sbxm::backend::SandboxDetail;
    use sbxm::status::sandboxes::{self, Local};
    let tmp = tempfile::tempdir().unwrap();
    save(tmp.path(), &task(Kind::Issue, 41, "Add titles"));
    save(tmp.path(), &task(Kind::Issue, 42, ""));
    let sandbox = |name: &str| SandboxDetail {
        name: name.to_owned(),
        status: "running".to_owned(),
        cpus: None,
        memory_mib: None,
        expires_at: None,
        created_at: None,
    };
    let local = Local::Sandboxes(vec![
        sandbox("sbxm-task-issue-41-claude"),
        sandbox("sbxm-task-issue-42-claude"),
    ]);

    let text = sandboxes::render(&local, &sandboxes::claims(tmp.path()), 0, None);

    assert!(text.contains("task issue-41 (Add titles)"), "{text}");
    assert!(text.contains("task issue-42  "), "{text}");
}

#[test]
fn the_loop_section_titles_its_task_and_the_prs_awaiting_but_not_one_whose_record_is_gone() {
    use sbxm::status::looping::{self, Current, Lock, Snapshot, View, Work};
    let mut work = Work {
        current: Some(Current {
            id: "issue-41".to_owned(),
            stage: "working",
        }),
        done: vec!["issue-40".to_owned()],
        ..Work::default()
    };
    work.titles
        .insert("issue-41".to_owned(), "Add titles".to_owned());
    work.titles
        .insert("issue-40".to_owned(), "Earlier".to_owned());
    let view = View {
        lock: Lock::Running(Process::new(1, T0)),
        work: Some(work),
        snapshot: Some(Snapshot {
            queue: Ok(Vec::new()),
            awaiting: Ok(vec![9, 13]),
            pr_titles: [(9, "Earlier".to_owned())].into_iter().collect(),
        }),
        pause: sbxm::task::loop_pause::State::None,
    };

    let text = looping::render(&view, None);

    assert!(
        text.contains("  task: issue-41 (Add titles) at working\n"),
        "{text}"
    );
    assert!(text.contains("  done: 1 (issue-40 (Earlier))\n"), "{text}");
    assert!(text.contains("PRs (#9: Earlier, #13)\n"), "{text}");
}
