//! Issue 725: `GhBackend::repo_paths` lists every path of a branch in one read of `gh api
//! repos/<repo>/git/trees/<branch>?recursive=1`, and refuses a listing GitHub cut short.

use std::sync::Mutex;

use anyhow::{Result, bail};
use sbxm::github::GitHubBackend;
use sbxm::github::gh::{GhBackend, GhRunner};

struct Scripted {
    calls: Mutex<Vec<Vec<String>>>,
    tree: &'static str,
}

impl GhRunner for &'static Scripted {
    fn run(&self, args: &[String], _stdin: Option<&str>) -> Result<String> {
        self.calls.lock().unwrap().push(args.to_vec());
        match args.get(1).map(String::as_str) {
            Some("repos/o/r/git/trees/main?recursive=1") => Ok(self.tree.to_owned()),
            _ => bail!("unexpected gh call {args:?}"),
        }
    }
}

fn backend(tree: &'static str) -> (GhBackend, &'static Scripted) {
    let script: &'static Scripted = Box::leak(Box::new(Scripted {
        calls: Mutex::default(),
        tree,
    }));
    (GhBackend::with_runner(Box::new(script)), script)
}

#[test]
fn github_repo_paths_lists_files_and_folders_in_one_read() {
    let (gh, script) = backend(
        r#"{"sha": "abc", "tree": [
            {"path": "README.md", "type": "blob"},
            {"path": "src", "type": "tree"},
            {"path": "src/cli.rs", "type": "blob"}
        ], "truncated": false}"#,
    );

    let paths = gh.repo_paths("o/r", "main").unwrap();

    assert_eq!(paths, ["README.md", "src", "src/cli.rs"]);
    assert_eq!(
        *script.calls.lock().unwrap(),
        [vec![
            "api".to_owned(),
            "repos/o/r/git/trees/main?recursive=1".to_owned()
        ]]
    );
}

#[test]
fn github_repo_paths_refuses_a_listing_cut_short() {
    let (gh, _) = backend(r#"{"tree": [{"path": "a.rs", "type": "blob"}], "truncated": true}"#);

    let err = gh.repo_paths("o/r", "main").unwrap_err();

    assert!(format!("{err:#}").contains("cut short"), "{err:#}");
}
