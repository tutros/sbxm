//! Issue 726: `GitHubBackend::prs_open` lists the numbers of a repository's open PRs in one read
//! of `gh pr list --state open`, and refuses a listing that reached its limit.

use std::sync::Mutex;

use anyhow::{Result, bail};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::gh::{GhBackend, GhRunner};
use sbxm::github::{GitHubBackend, ListedPr, PrState};

struct Scripted {
    calls: Mutex<Vec<Vec<String>>>,
    listing: String,
}

impl GhRunner for &'static Scripted {
    fn run(&self, args: &[String], _stdin: Option<&str>) -> Result<String> {
        self.calls.lock().unwrap().push(args.to_vec());
        match args.first().map(String::as_str) {
            Some("pr") => Ok(self.listing.clone()),
            _ => bail!("unexpected gh call {args:?}"),
        }
    }
}

fn backend(listing: String) -> (GhBackend, &'static Scripted) {
    let script: &'static Scripted = Box::leak(Box::new(Scripted {
        calls: Mutex::default(),
        listing,
    }));
    (GhBackend::with_runner(Box::new(script)), script)
}

fn listing(count: u32) -> String {
    let prs: Vec<String> = (1..=count)
        .map(|n| format!("{{\"number\": {n}}}"))
        .collect();
    format!("[{}]", prs.join(","))
}

#[test]
fn github_prs_open_lists_the_open_prs_numbers_in_one_read() {
    let (gh, script) = backend(r#"[{"number": 12}, {"number": 7}]"#.to_owned());

    assert_eq!(gh.prs_open("o/r").unwrap(), [7, 12]);
    let calls = script.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    let call = calls[0].join(" ");
    assert!(
        call.starts_with("pr list --repo o/r --state open --limit 100 --json number"),
        "{call}"
    );
}

#[test]
fn github_prs_open_one_under_the_limit_is_listed() {
    let (gh, _) = backend(listing(99));
    assert_eq!(gh.prs_open("o/r").unwrap().len(), 99);
}

#[test]
fn github_prs_open_a_listing_at_the_limit_is_refused() {
    let (gh, _) = backend(listing(100));
    let err = format!("{:#}", gh.prs_open("o/r").unwrap_err());
    assert!(err.contains("100"), "{err}");
}

fn listed(number: u32) -> ListedPr {
    ListedPr {
        number,
        url: format!("https://github.com/o/r/pull/{number}"),
        head_ref: format!("b{number}"),
        base_ref: "main".into(),
        is_cross_repository: false,
    }
}

#[test]
fn github_prs_open_the_fake_lists_its_open_prs_of_the_repo() {
    let gh = FakeGitHub::default()
        .with_listed_pr("o/r", listed(3), PrState::Open)
        .with_listed_pr("o/r", listed(4), PrState::Merged)
        .with_listed_pr("o/other", listed(5), PrState::Open)
        .with_listed_pr("o/r", listed(1), PrState::Open);

    assert_eq!(gh.prs_open("o/r").unwrap(), [1, 3]);
    assert_eq!(gh.calls(), [GhCall::PrsOpen("o/r".into())]);
}
