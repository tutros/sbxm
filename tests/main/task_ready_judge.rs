//! Issue 726: `sbxm task ready-check --issue N --judge [--write-files [--yes]]`, decision 331(2):
//! the model's answer and its parse, the `**Files:**` line it proposes written into the body, the
//! embedded prompt, and the one-shot sandbox run with its clean-up.

use sbxm::task::ready_judge::{self, SECTIONS};

/// An answer with every section.
const FULL: &str = "files: `src/a.rs`, `src/b.rs` (new)\n\
open_points:\n\
- \"It may or may not be cached.\": which one?\n\
boundary_cases: needed\n\
- an empty list\n\
acceptance_vs_answers: none\n\
size: fits\n";

// The answer's parse.

#[test]
fn ready_judge_an_answer_with_every_section_parses() {
    let answer = ready_judge::parse(FULL).unwrap();
    assert_eq!(answer.files, "`src/a.rs`, `src/b.rs` (new)");
    assert_eq!(
        answer.open_points,
        "- \"It may or may not be cached.\": which one?"
    );
    assert_eq!(answer.boundary_cases, "needed\n- an empty list");
    assert_eq!(answer.acceptance_vs_answers, "none");
    assert_eq!(answer.size, "fits");
}

#[test]
fn ready_judge_text_before_the_first_section_and_fence_lines_are_left_out() {
    let text = format!("Here is my answer.\n```\n{FULL}```\n");
    let answer = ready_judge::parse(&text).unwrap();
    assert_eq!(answer.files, "`src/a.rs`, `src/b.rs` (new)");
    assert_eq!(answer.size, "fits");
}

#[test]
fn ready_judge_crlf_line_endings_parse_the_same() {
    let answer = ready_judge::parse(&FULL.replace('\n', "\r\n")).unwrap();
    assert_eq!(answer, ready_judge::parse(FULL).unwrap());
}

#[test]
fn ready_judge_an_answer_with_one_section_missing_does_not_parse() {
    let text = FULL.replace("size: fits\n", "");
    let why = ready_judge::parse(&text).unwrap_err();
    assert!(why.contains("size"), "{why}");
}

#[test]
fn ready_judge_an_answer_not_in_the_format_does_not_parse() {
    let why = ready_judge::parse("The issue looks fine to me.").unwrap_err();
    for section in SECTIONS {
        assert!(why.contains(section), "{why}");
    }
}

#[test]
fn ready_judge_an_empty_section_does_not_parse() {
    let text = FULL.replace("acceptance_vs_answers: none", "acceptance_vs_answers:");
    let why = ready_judge::parse(&text).unwrap_err();
    assert!(why.contains("acceptance_vs_answers"), "{why}");
}

#[test]
fn ready_judge_a_section_given_twice_does_not_parse() {
    let why = ready_judge::parse(&format!("{FULL}size: M\n")).unwrap_err();
    assert!(why.contains("size") && why.contains("twice"), "{why}");
}

#[test]
fn ready_judge_files_that_are_not_backticked_paths_do_not_parse() {
    let text = FULL.replace("`src/a.rs`, `src/b.rs` (new)", "src/a.rs and src/b.rs");
    let why = ready_judge::parse(&text).unwrap_err();
    assert!(why.contains("files"), "{why}");
}

#[test]
fn ready_judge_files_on_two_lines_do_not_parse() {
    let text = FULL.replace("`src/b.rs` (new)", "\n`src/b.rs` (new)");
    let why = ready_judge::parse(&text).unwrap_err();
    assert!(why.contains("files"), "{why}");
}

#[test]
fn ready_judge_files_none_parses_and_proposes_no_line() {
    let text = FULL.replace("`src/a.rs`, `src/b.rs` (new)", "none");
    let answer = ready_judge::parse(&text).unwrap();
    assert_eq!(answer.files_line(), None);
}

#[test]
fn ready_judge_the_files_line_is_the_bold_label_and_the_paths() {
    let answer = ready_judge::parse(FULL).unwrap();
    assert_eq!(
        answer.files_line().as_deref(),
        Some("**Files:** `src/a.rs`, `src/b.rs` (new)")
    );
}

// The `**Files:**` line written into the body.

const LINE: &str = "**Files:** `src/a.rs`";

#[test]
fn ready_judge_a_body_with_no_files_line_gets_it_before_depends_on() {
    let body = "Text.\n\n**Depends on:** none\n\n## Acceptance criteria\n";
    assert_eq!(
        ready_judge::with_files_line(body, LINE),
        "Text.\n\n**Files:** `src/a.rs`\n\n**Depends on:** none\n\n## Acceptance criteria\n"
    );
}

#[test]
fn ready_judge_a_body_with_a_files_line_gets_it_replaced() {
    let body = "Text.\n\n**Files:** `old.rs`, `x.rs`\n\n**Depends on:** none\n";
    assert_eq!(
        ready_judge::with_files_line(body, LINE),
        "Text.\n\n**Files:** `src/a.rs`\n\n**Depends on:** none\n"
    );
}

#[test]
fn ready_judge_a_files_line_in_a_crlf_body_keeps_its_line_ending() {
    let body = "Text.\r\n**Files:** `old.rs`\r\n**Depends on:** none\r\n";
    assert_eq!(
        ready_judge::with_files_line(body, LINE),
        "Text.\r\n**Files:** `src/a.rs`\r\n**Depends on:** none\r\n"
    );
}

#[test]
fn ready_judge_lines_inside_a_code_fence_are_not_touched() {
    let body = "```\n**Files:** `x`\n**Depends on:** none\n```\n\n**Depends on:** #1\n";
    assert_eq!(
        ready_judge::with_files_line(body, LINE),
        "```\n**Files:** `x`\n**Depends on:** none\n```\n\n**Files:** `src/a.rs`\n\n**Depends on:** #1\n"
    );
}

#[test]
fn ready_judge_a_body_with_neither_line_gets_it_at_the_end() {
    assert_eq!(
        ready_judge::with_files_line("Text.", LINE),
        "Text.\n\n**Files:** `src/a.rs`\n"
    );
    assert_eq!(
        ready_judge::with_files_line("Text.\n", LINE),
        "Text.\n\n**Files:** `src/a.rs`\n"
    );
}

// The embedded prompt.

#[test]
fn ready_judge_the_prompt_asks_for_every_answer_section() {
    for section in SECTIONS {
        assert!(
            ready_judge::PROMPT.contains(&format!("{section}:")),
            "the prompt doesn't ask for `{section}:`"
        );
    }
}

#[test]
fn ready_judge_the_prompt_states_the_readiness_rules_and_names_its_inputs() {
    let prompt = ready_judge::PROMPT;
    for needle in [
        "boundary case",
        "**Files:**",
        ready_judge::ISSUE_FILE,
        ready_judge::PATHS_FILE,
        ready_judge::OPEN_PRS_FILE,
    ] {
        assert!(prompt.contains(needle), "the prompt doesn't name {needle}");
    }
}

#[test]
fn ready_judge_the_prompt_sizes_by_the_repositorys_table() {
    // The `sdlc-code-review` skill's "Size" table, section 8 (review S-1).
    let prompt = ready_judge::PROMPT;
    for needle in [
        "One module, no new state or file format, no open points",
        "One command or feature, with tests and docs across a few files",
        "The state machine, `task.json` or the git trust boundary changes, or three or more open \
         points",
    ] {
        assert!(prompt.contains(needle), "the prompt doesn't say: {needle}");
    }
    assert!(
        !prompt.contains("one or two files"),
        "the old size rule is gone"
    );
}

// The command: `sbxm task ready-check --issue N --judge [--write-files [--yes]]`.

use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::common::Env;
use crate::common::task_fixture::open_issue;
use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::commands::task_ready_check::{self, Judge};
use sbxm::config::Profile;
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::{Issue, IssueText, ListedPr, PrFile, PrFiles, PrState};
use serde_json::json;

/// A body every deterministic rule passes.
const READY_BODY: &str = "**Size:** M: a reason.\n\nChange `src/cli.rs`.\n\n\
**Depends on:** none\n\n\
## Acceptance criteria\n\n\
- [ ] It works\n";

/// A Claude `stream-json` run whose final answer is `text`.
fn claude_reply(text: &str) -> ExecOutput {
    let result = json!({
        "type": "result", "subtype": "success", "is_error": false,
        "result": text, "total_cost_usd": 0.25,
        "usage": {"input_tokens": 10, "output_tokens": 5,
                  "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}
    });
    ExecOutput {
        stdout: format!("{result}\n"),
        stderr: String::new(),
        exit_code: Some(0),
    }
}

/// An env whose profiles dir holds the `ready-check` profile.
fn env() -> Env {
    let env = Env::new();
    env.write_profile("ready-check", "description = \"model only\"\n");
    env
}

/// `gh` with issue 7 readable as `gh issue view` prints it, the default branch's files and one
/// open PR, #5, changing `src/cli.rs`.
fn github(body: &str) -> FakeGitHub {
    let issue: Issue = open_issue(7, &["ready-for-worker", "size:M"], body);
    FakeGitHub::default()
        .with_default_branch("main")
        .with_issue_text(IssueText {
            number: 7,
            title: issue.title.clone(),
            state: "OPEN".into(),
            text: format!(
                "title:\t{}\nstate:\tOPEN\nlabels:\tready-for-worker, size:M\n--\n{}",
                issue.title, issue.body
            ),
        })
        .with_issue_label_names(7, &["ready-for-worker", "size:M"])
        .with_issues(vec![issue.clone()])
        .with_repo_files(&[("src/cli.rs", ""), ("README.md", "")])
        .with_listed_pr(
            "o/r",
            ListedPr {
                number: 5,
                url: "https://github.com/o/r/pull/5".into(),
                head_ref: "issue-5".into(),
                base_ref: "main".into(),
                is_cross_repository: false,
            },
            PrState::Open,
        )
        .with_pr_files(
            5,
            PrFiles {
                changed_files: 1,
                files: vec![PrFile {
                    filename: "src/cli.rs".into(),
                    previous_filename: None,
                }],
            },
        )
}

fn opts() -> task_ready_check::Options {
    task_ready_check::Options {
        repo_root: std::env::temp_dir(),
        repo: Some("o/r".into()),
        issue: 7,
        json: false,
    }
}

fn judge(env: &Env, write_files: bool, yes: bool) -> Judge {
    Judge {
        config_dir: env.config_dir(),
        guard: sbxm::disk::Guard::default(),
        time_limit: std::time::Duration::from_secs(60),
        write_files,
        yes,
    }
}

struct Ran {
    result: anyhow::Result<i32>,
    out: String,
    warn: String,
}

fn ran(env: &Env, judge: &Judge, backend: &FakeBackend, gh: &FakeGitHub) -> Ran {
    let mut out = Vec::new();
    let mut warn = Vec::new();
    let result = task_ready_check::run_judged(&opts(), judge, backend, gh, &mut out, &mut warn);
    let _ = env;
    Ran {
        result,
        out: String::from_utf8(out).unwrap(),
        warn: String::from_utf8(warn).unwrap(),
    }
}

fn backend_answering(answer: &str) -> FakeBackend {
    FakeBackend::default().with_default_exec_output(claude_reply(answer))
}

/// The names of the folders in `dir`; none when it doesn't exist.
fn entries(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// Nothing the run made is left: no sandbox, no workspace, no kits.
fn assert_nothing_left(env: &Env, backend: &FakeBackend) {
    let created: Vec<String> = backend.creates().into_iter().map(|c| c.name).collect();
    assert_eq!(created.len(), 1, "{created:?}");
    assert_eq!(backend.removes(), created);
    let base = env.base_dir();
    assert!(entries(&base.join("ready-check")).is_empty());
    assert!(entries(&base.join(".sbxm").join("ready-check")).is_empty());
}

#[test]
fn ready_judge_a_ready_issue_prints_the_rules_the_answer_and_the_cost() {
    let env = env();
    let backend = backend_answering(FULL);
    let gh = github(READY_BODY);

    let ran = ran(&env, &judge(&env, false, false), &backend, &gh);

    assert_eq!(ran.result.unwrap(), 0, "{}{}", ran.out, ran.warn);
    assert!(ran.out.contains("pass size: "), "{}", ran.out);
    assert!(ran.out.contains("$0.25"), "{}", ran.out);
    assert!(
        ran.out
            .contains("files: `src/a.rs`, `src/b.rs` (new)\nopen_points: - \""),
        "{}",
        ran.out
    );
    assert!(ran.out.contains("size: fits"), "{}", ran.out);
    assert_nothing_left(&env, &backend);
}

#[test]
fn ready_judge_the_sandbox_is_claude_under_the_ready_check_profile_with_its_default_model() {
    let env = env();
    let backend = backend_answering(FULL);

    let ran = ran(
        &env,
        &judge(&env, false, false),
        &backend,
        &github(READY_BODY),
    );

    assert_eq!(ran.result.unwrap(), 0);
    let create = &backend.creates()[0];
    assert_eq!(create.agent, "claude");
    assert!(
        create.name.starts_with("sbxm-ready-issue-7-"),
        "{}",
        create.name
    );
    let (_, exec) = &backend.execs()[0];
    let argv = exec.argv.join(" ");
    assert!(
        argv.starts_with("timeout -v --kill-after=10 60 claude"),
        "{argv}"
    );
    assert!(!argv.contains("--model"), "{argv}");
}

#[test]
fn ready_judge_the_inputs_are_files_in_the_workspace() {
    let env = env();
    let seen: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let record = seen.clone();
    let backend = backend_answering(FULL).with_create_hook(move |spec| {
        let workspace = spec.workspace.clone().unwrap();
        for name in [
            "prompt.md",
            ready_judge::ISSUE_FILE,
            ready_judge::PATHS_FILE,
            ready_judge::OPEN_PRS_FILE,
        ] {
            let text = std::fs::read_to_string(workspace.join(name)).unwrap_or_default();
            record.lock().unwrap().push((name.to_owned(), text));
        }
    });

    let ran = ran(
        &env,
        &judge(&env, false, false),
        &backend,
        &github(READY_BODY),
    );

    assert_eq!(ran.result.unwrap(), 0);
    let seen = seen.lock().unwrap();
    let file = |name: &str| seen.iter().find(|(n, _)| n == name).unwrap().1.clone();
    assert_eq!(file("prompt.md"), ready_judge::PROMPT);
    assert!(file(ready_judge::ISSUE_FILE).contains("Change `src/cli.rs`."));
    assert!(file(ready_judge::ISSUE_FILE).contains("Fix 7"));
    assert_eq!(
        file(ready_judge::PATHS_FILE),
        "src\nsrc/cli.rs\nREADME.md\n"
    );
    let prs = file(ready_judge::OPEN_PRS_FILE);
    assert!(prs.contains("#5") && prs.contains("src/cli.rs"), "{prs}");
}

#[test]
fn ready_judge_the_repos_ready_check_profile_reaches_only_the_model_provider() {
    let profiles = Path::new(env!("CARGO_MANIFEST_DIR")).join("profiles");
    let profile = Profile::load(&profiles, "ready-check").unwrap();

    assert!(
        profile.network.allow.is_empty(),
        "{:?}",
        profile.network.allow
    );
    assert_eq!(profile.secrets.services, ["anthropic"]);
    assert!(profile.setup.install.is_empty());
    assert!(profile.template.is_none());
}

#[test]
fn ready_judge_an_answer_not_in_the_format_is_printed_raw_with_a_warning_and_exits_2() {
    let env = env();
    let backend = backend_answering("The issue looks fine to me.");
    let gh = github(READY_BODY);

    let ran = ran(&env, &judge(&env, true, true), &backend, &gh);

    assert_eq!(ran.result.unwrap(), 2);
    assert!(
        ran.out.contains("The issue looks fine to me."),
        "{}",
        ran.out
    );
    assert!(ran.warn.contains("warning:"), "{}", ran.warn);
    assert!(
        !gh.calls()
            .iter()
            .any(|c| matches!(c, GhCall::IssueEdit(..))),
        "nothing is written from an answer that doesn't parse"
    );
    assert_nothing_left(&env, &backend);
}

#[test]
fn ready_judge_an_answer_missing_a_section_exits_2() {
    let env = env();
    let backend = backend_answering(&FULL.replace("size: fits\n", ""));

    let ran = ran(
        &env,
        &judge(&env, false, false),
        &backend,
        &github(READY_BODY),
    );

    assert_eq!(ran.result.unwrap(), 2);
    assert!(ran.warn.contains("size"), "{}", ran.warn);
}

#[test]
fn ready_judge_a_failing_rule_exits_1_with_the_answer_printed() {
    let env = env();
    let backend = backend_answering(FULL);
    let body = READY_BODY.replace("**Depends on:** none\n", "");

    let ran = ran(&env, &judge(&env, false, false), &backend, &github(&body));

    assert_eq!(ran.result.unwrap(), 1);
    assert!(ran.out.contains("fail depends-on"), "{}", ran.out);
    assert!(ran.out.contains("size: fits"), "{}", ran.out);
}

#[test]
fn ready_judge_a_failed_create_leaves_no_sandbox_and_fails() {
    let env = env();
    let backend = FakeBackend::failing_create();

    let ran = ran(
        &env,
        &judge(&env, false, false),
        &backend,
        &github(READY_BODY),
    );

    let err = format!("{:#}", ran.result.unwrap_err());
    assert!(err.contains("cannot create"), "{err}");
    assert!(backend.execs().is_empty());
    assert_nothing_left(&env, &backend);
}

#[test]
fn ready_judge_a_timed_out_run_leaves_no_sandbox_and_fails() {
    let env = env();
    let backend = FakeBackend::default().with_default_exec_output(ExecOutput {
        stdout: String::new(),
        stderr: "timeout: sending signal TERM to command 'claude'\n".into(),
        exit_code: Some(124),
    });

    let ran = ran(
        &env,
        &judge(&env, false, false),
        &backend,
        &github(READY_BODY),
    );

    let err = format!("{:#}", ran.result.unwrap_err());
    assert!(err.contains("timed out after 60s"), "{err}");
    assert_nothing_left(&env, &backend);
}

#[test]
fn ready_judge_a_sandbox_that_cannot_be_removed_is_named_in_the_error() {
    let env = env();
    let backend = backend_answering(FULL).and_failing_remove();

    let ran = ran(
        &env,
        &judge(&env, false, false),
        &backend,
        &github(READY_BODY),
    );

    let err = format!("{:#}", ran.result.unwrap_err());
    assert!(err.contains("sbx rm -f sbxm-ready-issue-7-"), "{err}");
}

#[test]
fn ready_judge_a_missing_profile_fails_before_any_sandbox() {
    let env = Env::new();
    let backend = backend_answering(FULL);

    let ran = ran(
        &env,
        &judge(&env, false, false),
        &backend,
        &github(READY_BODY),
    );

    assert!(ran.result.is_err());
    assert!(backend.creates().is_empty());
    // Checked before the first write: not even a parent folder is made.
    assert!(!env.base_dir().join("ready-check").exists());
    assert!(!env.base_dir().join(".sbxm").exists());
}

#[test]
fn ready_judge_github_reads_that_fail_stop_it_before_any_sandbox() {
    let env = env();
    let backend = backend_answering(FULL);
    // PR #5 is listed open but its files can't be read.
    let gh = github(READY_BODY).with_listed_pr(
        "o/r",
        ListedPr {
            number: 9,
            url: String::new(),
            head_ref: "x".into(),
            base_ref: "main".into(),
            is_cross_repository: false,
        },
        PrState::Open,
    );

    let ran = ran(&env, &judge(&env, false, false), &backend, &gh);

    let err = format!("{:#}", ran.result.unwrap_err());
    assert!(err.contains("#9"), "{err}");
    assert!(backend.creates().is_empty());
}

// `--write-files`.

fn edits(gh: &FakeGitHub) -> Vec<(u32, String)> {
    gh.calls()
        .into_iter()
        .filter_map(|c| match c {
            GhCall::IssueEdit(_, number, body) => Some((number, body)),
            _ => None,
        })
        .collect()
}

#[test]
fn ready_judge_write_files_adds_the_line_before_depends_on() {
    let env = env();
    let gh = github(READY_BODY);

    let ran = ran(
        &env,
        &judge(&env, true, true),
        &backend_answering(FULL),
        &gh,
    );

    assert_eq!(ran.result.unwrap(), 0, "{}", ran.warn);
    let edits = edits(&gh);
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].0, 7);
    assert!(
        edits[0]
            .1
            .contains("**Files:** `src/a.rs`, `src/b.rs` (new)\n\n**Depends on:** none\n"),
        "{}",
        edits[0].1
    );
    assert!(ran.out.contains("**Files:** `src/a.rs`"), "{}", ran.out);
}

#[test]
fn ready_judge_write_files_replaces_an_old_files_line() {
    let env = env();
    let body = READY_BODY.replace("**Depends on:**", "**Files:** `old.rs`\n\n**Depends on:**");
    let gh = github(&body);

    let ran = ran(
        &env,
        &judge(&env, true, true),
        &backend_answering(FULL),
        &gh,
    );

    assert_eq!(ran.result.unwrap(), 0, "{}", ran.warn);
    let edits = edits(&gh);
    assert_eq!(edits.len(), 1);
    assert!(!edits[0].1.contains("old.rs"), "{}", edits[0].1);
    assert_eq!(
        edits[0].1.matches("**Files:**").count(),
        1,
        "{}",
        edits[0].1
    );
}

#[test]
fn ready_judge_write_files_without_yes_writes_nothing_and_prints_the_command() {
    let env = env();
    let gh = github(READY_BODY);

    let ran = ran(
        &env,
        &judge(&env, true, false),
        &backend_answering(FULL),
        &gh,
    );

    assert_eq!(ran.result.unwrap(), 0);
    assert!(edits(&gh).is_empty());
    assert!(ran.out.contains("**Files:** `src/a.rs`"), "{}", ran.out);
    assert!(
        ran.out
            .contains("sbxm task ready-check --issue 7 --judge --write-files --yes"),
        "{}",
        ran.out
    );
}

#[test]
fn ready_judge_without_write_files_nothing_is_written() {
    let env = env();
    let gh = github(READY_BODY);

    let ran = ran(
        &env,
        &judge(&env, false, true),
        &backend_answering(FULL),
        &gh,
    );

    assert_eq!(ran.result.unwrap(), 0);
    assert!(edits(&gh).is_empty());
}

#[test]
fn ready_judge_write_files_with_no_proposed_line_writes_nothing() {
    let env = env();
    let gh = github(READY_BODY);
    let answer = FULL.replace("`src/a.rs`, `src/b.rs` (new)", "none");

    let ran = ran(
        &env,
        &judge(&env, true, true),
        &backend_answering(&answer),
        &gh,
    );

    assert_eq!(ran.result.unwrap(), 0);
    assert!(edits(&gh).is_empty());
}

// The flags.

fn sbxm_fails(args: &[&str]) -> String {
    let output = assert_cmd::Command::cargo_bin("sbxm")
        .unwrap()
        .args(args)
        .assert()
        .failure()
        .get_output()
        .clone();
    String::from_utf8(output.stderr).unwrap()
}

#[test]
fn ready_judge_write_files_needs_judge() {
    let err = sbxm_fails(&["task", "ready-check", "--issue", "7", "--write-files"]);
    assert!(err.contains("--judge"), "{err}");
}

#[test]
fn ready_judge_yes_needs_write_files() {
    let err = sbxm_fails(&["task", "ready-check", "--issue", "7", "--judge", "--yes"]);
    assert!(err.contains("--write-files"), "{err}");
}

#[test]
fn ready_judge_judge_does_not_go_with_json() {
    let err = sbxm_fails(&["task", "ready-check", "--issue", "7", "--judge", "--json"]);
    assert!(err.contains("--json") && err.contains("--judge"), "{err}");
}

#[test]
fn ready_judge_a_secret_the_profile_names_but_sbx_lacks_fails_before_any_sandbox() {
    let env = env();
    env.write_profile(
        "ready-check",
        "description = \"model only\"\n\n[secrets]\nservices = [\"anthropic\"]\n",
    );
    let backend = backend_answering(FULL);

    let refused = ran(
        &env,
        &judge(&env, false, false),
        &backend,
        &github(READY_BODY),
    );

    let err = format!("{:#}", refused.result.unwrap_err());
    assert!(err.contains("secret 'anthropic'"), "{err}");
    assert!(backend.creates().is_empty());
    assert!(!env.base_dir().join(".sbxm").exists());

    let backend = backend_answering(FULL).and_secrets(&["anthropic"]);
    let stored = ran(
        &env,
        &judge(&env, false, false),
        &backend,
        &github(READY_BODY),
    );
    assert_eq!(stored.result.unwrap(), 0);
}

// The machine's sandbox slot (decision 304): held from before the create until the clean-up ends.

/// Runs the judge with `sandbox_slots = 1` and `backend`, recording whether the slot was held
/// when the sandbox's removal began; the slot is free again once the command returns.
fn slot_during_removal(env: &Env, backend: FakeBackend) -> (Ran, Vec<bool>) {
    use crate::common::slot;
    let held: Arc<Mutex<Vec<bool>>> = Arc::default();
    let record = held.clone();
    let file = slot::slot_file(env);
    let backend = backend.with_remove_hook(move |_| record.lock().unwrap().push(file.exists()));
    let judge = Judge {
        guard: slot::free(env),
        ..judge(env, false, false)
    };
    let result = ran(env, &judge, &backend, &github(READY_BODY));
    assert!(
        slot::holder(env).is_none(),
        "the slot is given back at the end"
    );
    let held = held.lock().unwrap().clone();
    (result, held)
}

#[test]
fn ready_judge_the_slot_is_held_while_a_completed_runs_sandbox_is_removed() {
    let env = env();
    let (ran, held) = slot_during_removal(&env, backend_answering(FULL));
    assert_eq!(ran.result.unwrap(), 0);
    assert_eq!(held, [true]);
}

#[test]
fn ready_judge_the_slot_is_held_while_a_failed_creates_sandbox_is_removed() {
    let env = env();
    let (ran, held) = slot_during_removal(&env, FakeBackend::failing_create());
    assert!(ran.result.is_err());
    assert_eq!(held, [true]);
}

#[test]
fn ready_judge_the_slot_is_held_while_a_timed_out_runs_sandbox_is_removed() {
    let env = env();
    let backend = FakeBackend::default().with_default_exec_output(ExecOutput {
        stdout: String::new(),
        stderr: "timeout: sending signal TERM to command 'claude'\n".into(),
        exit_code: Some(124),
    });
    let (ran, held) = slot_during_removal(&env, backend);
    assert!(ran.result.is_err());
    assert_eq!(held, [true]);
}
