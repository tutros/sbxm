//! Issue 721: `prs_merged_since` lists every merged PR, not only the first `--limit` of them.
//! When `gh pr list` returns as many PRs as the limit it asked for, it asks again with a doubled
//! limit, until fewer come back; the retro alert's count in status and alerts is then complete,
//! and PRs merged on the window's end day stay left out.

use std::sync::Mutex;

use anyhow::{Result, bail};
use sbxm::github::GitHubBackend;
use sbxm::github::gh::{GhBackend, GhRunner};
use sbxm::task::retro::{self, Due};

const END: &str = "2026-11-16";
/// PRs merged on the window's end day: `gh pr list` lists them (merged:>=end), the count doesn't.
const ON_END_DAY: usize = 3;

/// A `gh` that knows `qualifying` PRs merged after the end day and [`ON_END_DAY`] on it, and
/// gives `gh pr list` at most `--limit` of them, newest first, as GitHub does.
struct Scripted {
    qualifying: usize,
    limits: Mutex<Vec<usize>>,
}

impl Scripted {
    fn merged_prs(&self) -> Vec<String> {
        let after = (0..self.qualifying).map(|i| {
            let n = 10_000 - i;
            // Newest first: later hours on later days.
            format!(
                r#"{{"number": {n}, "mergedAt": "2026-12-{:02}T{:02}:00:00Z"}}"#,
                28 - i / 24 % 28,
                23 - i % 24
            )
        });
        let on_end = (0..ON_END_DAY).map(|i| {
            format!(
                r#"{{"number": {}, "mergedAt": "{END}T{:02}:00:00Z"}}"#,
                100 + i,
                23 - i
            )
        });
        after.chain(on_end).collect()
    }
}

impl GhRunner for &'static Scripted {
    fn run(&self, args: &[String], _stdin: Option<&str>) -> Result<String> {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        match args.as_slice() {
            ["repo", "view", ..] => Ok(r#"{"defaultBranchRef": {"name": "main"}}"#.into()),
            ["api", "repos/o/r/contents/sdlc/retros?ref=main"] => {
                Ok(r#"[{"name": "2026-11-16-retro.md", "type": "file"}]"#.into())
            }
            [
                "api",
                "repos/o/r/contents/sdlc/retros/2026-11-16-retro.md?ref=main",
                ..,
            ] => Ok(format!("Window: 2026-11-01 to {END}\n")),
            ["pr", "list", ..] => {
                let at = args.iter().position(|a| *a == "--limit").unwrap();
                let limit: usize = args[at + 1].parse().unwrap();
                self.limits.lock().unwrap().push(limit);
                let prs = self.merged_prs();
                Ok(format!("[{}]", prs[..limit.min(prs.len())].join(", ")))
            }
            _ => bail!("unexpected gh call: {args:?}"),
        }
    }
}

fn backend(qualifying: usize) -> (GhBackend, &'static Scripted) {
    let script: &'static Scripted = Box::leak(Box::new(Scripted {
        qualifying,
        limits: Mutex::default(),
    }));
    let gh = GhBackend::with_runner(Box::new(script)).retrying_with(|_| {}, |_| {});
    (gh, script)
}

fn due(qualifying: usize) -> Due {
    let (gh, _) = backend(qualifying);
    retro::check(&gh, "o/r").unwrap().unwrap()
}

#[test]
fn github_retro_limit_499_500_and_501_prs_are_each_counted_in_full() {
    for n in [499, 500, 501] {
        let due = due(n);
        assert_eq!(
            due,
            Due::Prs {
                prs: n,
                end: END.into()
            }
        );
        assert_eq!(due.status_line(), format!("retro due: {n} PRs since {END}"));
        assert_eq!(
            due.comment(),
            format!("Retro due: {n} PRs merged since {END}.")
        );
    }
}

#[test]
fn github_retro_limit_end_day_prs_stay_out_when_more_than_one_page_is_needed() {
    // With the 3 on the end day, 497 lists exactly the first limit of 500 and 498 one over it.
    for n in [497, 498, 1_000, 1_001] {
        assert_eq!(
            due(n),
            Due::Prs {
                prs: n,
                end: END.into()
            }
        );
    }
}

#[test]
fn github_retro_limit_every_listed_pr_comes_back_and_the_limit_doubles_until_short() {
    let (gh, script) = backend(1_200);

    let prs = gh.prs_merged_since("o/r", END).unwrap();

    assert_eq!(prs.len(), 1_200 + ON_END_DAY);
    assert_eq!(*script.limits.lock().unwrap(), [500, 1_000, 2_000]);
}

#[test]
fn github_retro_limit_one_call_when_fewer_than_the_limit_come_back() {
    let (gh, script) = backend(10);

    assert_eq!(
        gh.prs_merged_since("o/r", END).unwrap().len(),
        10 + ON_END_DAY
    );
    assert_eq!(*script.limits.lock().unwrap(), [500]);
}
