//! Issue 714: what the retro alert reads of GitHub through `GhBackend`: the PRs merged since a day
//! (`gh pr list --state merged --search merged:>=<day>`), a folder's files and a file's text on a
//! branch (`gh api repos/<repo>/contents/<path>?ref=<branch>`).

use std::sync::Mutex;

use anyhow::{Result, bail};
use sbxm::github::gh::{GhBackend, GhRunner};
use sbxm::github::{GitHubBackend, MergedPr};

struct Scripted {
    calls: Mutex<Vec<Vec<String>>>,
    /// What every call prints.
    out: String,
    failure: Option<&'static str>,
}

impl GhRunner for &'static Scripted {
    fn run(&self, args: &[String], _stdin: Option<&str>) -> Result<String> {
        self.calls.lock().unwrap().push(args.to_vec());
        if let Some(message) = self.failure {
            bail!("{message}");
        }
        Ok(self.out.clone())
    }
}

fn backend(out: &str, failure: Option<&'static str>) -> (GhBackend, &'static Scripted) {
    let script: &'static Scripted = Box::leak(Box::new(Scripted {
        calls: Mutex::default(),
        out: out.to_owned(),
        failure,
    }));
    let gh = GhBackend::with_runner(Box::new(script)).retrying_with(|_| {}, |_| {});
    (gh, script)
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|a| (*a).to_owned()).collect()
}

#[test]
fn github_retro_merged_prs_are_listed_from_the_day_with_their_merge_times() {
    let (gh, script) = backend(
        r#"[{"number": 700, "mergedAt": "2026-11-17T08:00:00Z"}]"#,
        None,
    );

    let prs = gh.prs_merged_since("o/r", "2026-11-16").unwrap();

    assert_eq!(
        prs,
        [MergedPr {
            number: 700,
            merged_at: sbxm::run::results::parse_timestamp("2026-11-17T08:00:00Z").unwrap(),
        }]
    );
    assert_eq!(
        *script.calls.lock().unwrap(),
        [args(&[
            "pr",
            "list",
            "--repo",
            "o/r",
            "--state",
            "merged",
            "--search",
            "merged:>=2026-11-16",
            "--limit",
            "500",
            "--json",
            "number,mergedAt",
        ])]
    );
}

#[test]
fn github_retro_a_day_that_isnt_a_date_is_refused_before_any_call() {
    let (gh, script) = backend("[]", None);

    assert!(gh.prs_merged_since("o/r", "2026-11-16 is:open").is_err());
    assert!(script.calls.lock().unwrap().is_empty());
}

#[test]
fn github_retro_a_folders_files_are_listed_on_the_branch() {
    let (gh, script) = backend(
        r#"[{"name": "2026-11-16-retro.md", "type": "file"}, {"name": "old", "type": "dir"}]"#,
        None,
    );

    let files = gh.dir_files("o/r", "main", "sdlc/retros").unwrap();

    assert_eq!(files, ["2026-11-16-retro.md"]);
    assert_eq!(
        *script.calls.lock().unwrap(),
        [args(&["api", "repos/o/r/contents/sdlc/retros?ref=main"])]
    );
}

#[test]
fn github_retro_a_missing_folder_has_no_files() {
    let (gh, _) = backend("", Some("gh: Not Found (HTTP 404)"));

    assert!(
        gh.dir_files("o/r", "main", "sdlc/retros")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn github_retro_another_failure_listing_a_folder_is_an_error() {
    let (gh, _) = backend("", Some("HTTP 403: rate limit"));
    assert!(gh.dir_files("o/r", "main", "sdlc/retros").is_err());

    let (gh, _) = backend("not json", None);
    assert!(gh.dir_files("o/r", "main", "sdlc/retros").is_err());
}

#[test]
fn github_retro_a_files_text_is_read_raw_on_the_branch() {
    let (gh, script) = backend("Window: 2026-11-15 to 2026-11-16\n", None);

    let text = gh
        .file_text("o/r", "main", "sdlc/retros/2026-11-16-retro.md")
        .unwrap();

    assert_eq!(text, "Window: 2026-11-15 to 2026-11-16\n");
    assert_eq!(
        *script.calls.lock().unwrap(),
        [args(&[
            "api",
            "repos/o/r/contents/sdlc/retros/2026-11-16-retro.md?ref=main",
            "-H",
            "Accept: application/vnd.github.raw+json",
        ])]
    );
}
