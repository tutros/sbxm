//! Issue 549: `GhBackend::pr_files` reads every page of `gh api repos/<repo>/pulls/<n>/files
//! --paginate` and the PR's `changed_files` count, for the medium-risk arming check.

use std::sync::Mutex;

use anyhow::{Result, bail};
use sbxm::github::gh::{GhBackend, GhRunner};
use sbxm::github::{GitHubBackend, PrFile};

struct Scripted {
    calls: Mutex<Vec<Vec<String>>>,
    /// What `gh api repos/o/r/pulls/12/files --paginate` prints.
    files: String,
    failure: Option<&'static str>,
}

impl GhRunner for &'static Scripted {
    fn run(&self, args: &[String], _stdin: Option<&str>) -> Result<String> {
        self.calls.lock().unwrap().push(args.to_vec());
        if let Some(message) = self.failure {
            bail!("{message}");
        }
        match args.get(1).map(String::as_str) {
            Some("repos/o/r/pulls/12/files") => Ok(self.files.clone()),
            Some("repos/o/r/pulls/12") => Ok(r#"{"number": 12, "changed_files": 101}"#.to_owned()),
            _ => bail!("unexpected gh call {args:?}"),
        }
    }
}

fn backend(files: String, failure: Option<&'static str>) -> (GhBackend, &'static Scripted) {
    let script: &'static Scripted = Box::leak(Box::new(Scripted {
        calls: Mutex::default(),
        files,
        failure,
    }));
    (GhBackend::with_runner(Box::new(script)), script)
}

fn page(names: impl Iterator<Item = String>) -> String {
    let entries: Vec<String> = names
        .map(|name| format!(r#"{{"filename": "{name}", "status": "modified", "additions": 1}}"#))
        .collect();
    format!("[\n{}\n]", entries.join(",\n"))
}

#[test]
fn every_page_is_read_and_the_changed_files_count_comes_with_it() {
    // 101 files: a first page of 100, then one more, printed one after the other.
    let first = page((0..100).map(|i| format!("src/f{i}.rs")));
    let second = page(std::iter::once("Cargo.lock".to_owned()));
    let (gh, script) = backend(format!("{first}{second}"), None);

    let files = gh.pr_files("o/r", 12).unwrap();

    assert_eq!(files.changed_files, 101);
    assert_eq!(files.files.len(), 101);
    assert_eq!(files.files[0].filename, "src/f0.rs");
    assert_eq!(
        files.files[100],
        PrFile {
            filename: "Cargo.lock".to_owned(),
            previous_filename: None,
        }
    );
    let calls = script.calls.lock().unwrap();
    assert!(
        calls.contains(&vec![
            "api".to_owned(),
            "repos/o/r/pulls/12/files".to_owned(),
            "--paginate".to_owned(),
        ]),
        "{calls:?}"
    );
}

#[test]
fn a_renamed_files_old_path_is_kept() {
    let text = r#"[{"filename": "Cargo.lock.old", "previous_filename": "Cargo.lock", "status": "renamed"}]"#;
    let (gh, _) = backend(text.to_owned(), None);

    let files = gh.pr_files("o/r", 12).unwrap();

    assert_eq!(
        files.files,
        [PrFile {
            filename: "Cargo.lock.old".to_owned(),
            previous_filename: Some("Cargo.lock".to_owned()),
        }]
    );
}

#[test]
fn output_that_is_not_a_file_list_is_an_error() {
    let (gh, _) = backend(r#"[{"filename": "a"}]{"message": "oops"}"#.to_owned(), None);

    let message = format!("{:#}", gh.pr_files("o/r", 12).unwrap_err());

    assert!(
        message.contains("gh api repos/o/r/pulls/12/files"),
        "{message}"
    );
}

#[test]
fn a_gh_failure_is_an_error() {
    let (gh, _) = backend(String::new(), Some("HTTP 404: Not Found"));

    let message = format!("{:#}", gh.pr_files("o/r", 12).unwrap_err());

    assert!(message.contains("404"), "{message}");
}
