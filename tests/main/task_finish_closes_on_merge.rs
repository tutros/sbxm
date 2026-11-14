//! Issue 506: an issue whose body has the line `**Closes on merge:** no` (the value in any case)
//! asks for its PR to leave it open. `task finish` then writes `Refs #N` in the PR description,
//! with no closing keyword, and says so; the worker prompt tells the worker to write `Refs #N` in
//! its commits. Any other value, no line, or the line inside a code fence keeps `Fixes #N`.

use crate::common;

use std::fs;

use common::task_fixture::{
    CLAUDE_DONE, Fixture, Play, Probe, backend, ctx, fixture, issue_text, ok, play, source,
    worked_task,
};
use sbxm::commands::task_finish::{Options, Target, run};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::{ListedPr, PrRequest, PrState};
use sbxm::task::closes::leaves_open;
use sbxm::task::pipeline::{self, Prepared};
use sbxm::task::record::{self, Process, Stage, Status};

/// `gh issue view` output with `body` under its header.
fn issue(body: &str) -> String {
    format!("title:\tFix 41\nstate:\tOPEN\nlabels:\tsize:S\n--\n{body}")
}

// --- The line ----------------------------------------------------------------------------------

#[test]
fn no_line_closes_the_issue() {
    assert!(!leaves_open(&issue("**Size:** S\n\nDo it.\n")));
    assert!(!leaves_open(""));
}

#[test]
fn no_in_any_case_leaves_the_issue_open() {
    for value in ["no", "No", "NO", "nO"] {
        let text = issue(&format!(
            "**Size:** S\n\n**Closes on merge:** {value}\n\nDo it.\n"
        ));
        assert!(leaves_open(&text), "{value}");
    }
}

#[test]
fn spaces_around_the_value_and_a_crlf_line_still_leave_it_open() {
    assert!(leaves_open(&issue(
        "**Closes on merge:**   no  \r\nDo it.\r\n"
    )));
    assert!(leaves_open(&issue("**Closes on merge:**no")));
}

#[test]
fn yes_or_another_value_closes_the_issue() {
    for value in [
        "yes",
        "Yes",
        "",
        "no.",
        "not yet",
        "no, later",
        "never",
        "false",
        "n",
    ] {
        let text = issue(&format!("**Closes on merge:** {value}\n"));
        assert!(!leaves_open(&text), "{value:?}");
    }
}

#[test]
fn the_line_inside_a_code_fence_is_ignored() {
    for fence in ["```", "~~~", "````"] {
        let text = issue(&format!(
            "Write it like this:\n\n{fence}\n**Closes on merge:** no\n{fence}\n"
        ));
        assert!(!leaves_open(&text), "{fence}");
    }
    // A fence that never closes hides the rest of the body.
    assert!(!leaves_open(&issue("```md\n**Closes on merge:** no\n")));
    // After the fence closes, the line counts again.
    assert!(leaves_open(&issue(
        "```\nexample\n```\n\n**Closes on merge:** no\n"
    )));
}

#[test]
fn only_a_line_starting_with_the_label_counts() {
    for line in [
        "  **Closes on merge:** no",
        "- **Closes on merge:** no",
        "> **Closes on merge:** no",
        "See `**Closes on merge:** no` for the form.",
        "Closes on merge: no",
    ] {
        assert!(!leaves_open(&issue(&format!("{line}\n"))), "{line}");
    }
}

#[test]
fn the_header_is_not_the_body() {
    // A title that quotes the line doesn't count: only the body after `--` does.
    let text = "title:\t**Closes on merge:** no\nstate:\tOPEN\n--\nDo it.\n";
    assert!(!leaves_open(text));
}

// --- The finish --------------------------------------------------------------------------------

/// Issue 41 worked, its gates and review done by hand: `ready`, its saved issue text `text`.
fn ready(f: &Fixture, text: &str) -> Prepared {
    let b = backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(play(
            &f.env.base_dir().join("tasks").join("issue-41"),
            "main",
            "issue-41",
            Play {
                commits: vec!["a.txt".into()],
                result_md: Some(b"done\n".to_vec()),
                bundle_bytes: None,
            },
        ));
    let mut prepared = worked_task(f, &b);
    let p = || Process::new(1, 0);
    let r = &mut prepared.record;
    r.begin_gating(1, p()).unwrap();
    r.finish(Status::Passed).unwrap();
    r.begin_review(2, p()).unwrap();
    r.finish(Status::Completed).unwrap();
    r.advance(Stage::Ready, 3, p()).unwrap();
    record::write(&prepared.meta, &prepared.record).unwrap();
    fs::write(prepared.meta.join("review.md"), "Must-fix findings: 0\n").unwrap();
    fs::write(prepared.meta.join("issue.md"), text).unwrap();
    prepared
}

fn go(f: &Fixture, github: &FakeGitHub) -> (anyhow::Result<()>, String) {
    let mut out = Vec::new();
    let result = run(
        &f.env.config_dir(),
        &Options {
            target: Target::Issue(41),
            repo_root: f.env.tmp.path().join("target-repo"),
            push_unresolved: false,
        },
        &sbxm::backend::FakeBackend::default(),
        github,
        &Probe,
        &mut out,
    );
    (result, String::from_utf8(out).unwrap())
}

fn created(github: &FakeGitHub) -> PrRequest {
    github
        .calls()
        .into_iter()
        .find_map(|c| match c {
            GhCall::PrCreate(_, request) => Some(request),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{:?}", github.calls()))
}

/// GitHub's closing keywords, each followed by a reference to issue 41 as GitHub reads one.
fn closing_references(body: &str) -> Vec<String> {
    let lower = body.to_lowercase();
    let mut found = Vec::new();
    for keyword in [
        "close", "closes", "closed", "fix", "fixes", "fixed", "resolve", "resolves", "resolved",
    ] {
        for form in [format!("{keyword} #41"), format!("{keyword}: #41")] {
            if lower.contains(&form) {
                found.push(form);
            }
        }
    }
    found
}

#[test]
fn an_issue_without_the_line_is_fixed_by_its_pr() {
    let f = fixture();
    ready(&f, &issue("Do it.\n"));
    let github = FakeGitHub::default();

    let (result, out) = go(&f, &github);

    result.unwrap();
    let body = created(&github).body;
    assert!(body.lines().any(|l| l == "Fixes #41"), "{body}");
    assert!(!body.contains("Refs #41"), "{body}");
    assert!(!out.contains("leaves #41 open"), "{out}");
}

#[test]
fn an_issue_that_stays_open_is_referred_to_and_the_output_says_so() {
    for value in ["no", "No", "NO"] {
        let f = fixture();
        ready(
            &f,
            &issue(&format!("**Closes on merge:** {value}\n\nDo it.\n")),
        );
        let github = FakeGitHub::default();

        let (result, out) = go(&f, &github);

        result.unwrap();
        let body = created(&github).body;
        assert!(body.lines().any(|l| l == "Refs #41"), "{value}: {body}");
        assert_eq!(closing_references(&body), Vec::<String>::new(), "{body}");
        assert!(
            out.contains(
                "  the PR leaves #41 open (Closes on merge: no): its description refers to it \
                 with Refs #41\n"
            ),
            "{value}: {out}"
        );
        assert!(!out.contains("left unchanged"), "{value}: {out}");
    }
}

#[test]
fn yes_or_another_value_is_fixed_by_its_pr() {
    for value in ["yes", "later"] {
        let f = fixture();
        ready(&f, &issue(&format!("**Closes on merge:** {value}\n")));
        let github = FakeGitHub::default();

        go(&f, &github).0.unwrap();

        let body = created(&github).body;
        assert!(body.lines().any(|l| l == "Fixes #41"), "{value}: {body}");
        assert!(!body.contains("Refs #41"), "{value}: {body}");
    }
}

#[test]
fn the_line_in_a_code_block_is_fixed_by_its_pr() {
    let f = fixture();
    ready(&f, &issue("Form:\n\n```\n**Closes on merge:** no\n```\n"));
    let github = FakeGitHub::default();

    go(&f, &github).0.unwrap();

    let body = created(&github).body;
    assert!(body.lines().any(|l| l == "Fixes #41"), "{body}");
}

#[test]
fn a_task_folder_without_its_issue_text_is_fixed_by_its_pr() {
    let f = fixture();
    let prepared = ready(&f, "");
    fs::remove_file(prepared.meta.join("issue.md")).unwrap();
    let github = FakeGitHub::default();

    go(&f, &github).0.unwrap();

    let body = created(&github).body;
    assert!(body.lines().any(|l| l == "Fixes #41"), "{body}");
}

// --- An adopted PR (issue 521) -----------------------------------------------------------------

/// An adopted PR's description is neither read nor changed, so the output doesn't claim it says
/// `Refs #41`: it says the description was left unchanged and asks the user to check it.
#[test]
fn an_adopted_pr_of_an_issue_that_stays_open_is_not_claimed_to_refer_to_it() {
    let f = fixture();
    ready(&f, &issue("**Closes on merge:** no\n\nDo it.\n"));
    let pr = ListedPr {
        number: 9,
        url: "https://github.com/o/r/pull/9".into(),
        head_ref: "issue-41".into(),
        base_ref: "main".into(),
        is_cross_repository: false,
    };
    let github = FakeGitHub::default().with_listed_pr("o/r", pr, PrState::Open);

    let (result, out) = go(&f, &github);

    result.unwrap();
    assert!(out.contains("already open for it"), "{out}");
    assert!(!out.contains("Refs #41"), "{out}");
    assert!(!out.contains("leaves #41 open"), "{out}");
    assert!(
        out.contains(
            "  the PR's description was left unchanged (an existing PR was adopted): check that \
             it refers to #41 with Refs, not Fixes, or the merge will close it\n"
        ),
        "{out}"
    );
    let calls = github.calls();
    assert!(
        !calls.iter().any(|c| matches!(c, GhCall::PrCreate(..))),
        "{calls:?}"
    );
}

// --- The worker prompt -------------------------------------------------------------------------

fn worker_prompt(text: &str) -> String {
    let f = fixture();
    let (backend, github) = (backend(), FakeGitHub::default());
    let source = source(&f);
    let mut issue = issue_text(41);
    issue.text = text.to_owned();
    let prepared = pipeline::prepare(&ctx(&f, &source, &backend, &github), &issue).unwrap();
    fs::read_to_string(prepared.meta.join("worker-prompt.md")).unwrap()
}

#[test]
fn the_worker_of_an_issue_that_stays_open_is_told_to_write_refs() {
    let prompt = worker_prompt(&issue("**Closes on merge:** no\n\nDo it.\n"));

    assert!(prompt.contains("\"Refs #41\""), "{prompt}");
    assert!(!prompt.contains("\"Fixes #41\""), "{prompt}");
}

#[test]
fn the_worker_of_any_other_issue_is_told_to_write_fixes() {
    let prompt = worker_prompt(&issue("Do it.\n"));

    assert!(
        prompt.contains("The last commit message must contain \"Fixes #41\"."),
        "{prompt}"
    );
    assert!(!prompt.contains("Refs #41"), "{prompt}");
}
