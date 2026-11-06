//! One file per decision (issue 433, decision 303): every numbered decision is
//! `sdlc/decisions/NNNN-<slug>.md`, so two branches that each add one don't conflict at the end
//! of a shared file. The folder is the index; nothing generated is checked in. These tests check
//! the file names, that no number is used twice, and that every reference to a decision in the
//! repo's tracked `.md` and `.rs` files resolves to a file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use regex::Regex;

/// Numbers known to have no file, each with its reason. Any other number without a file fails.
const UNWRITTEN: &[(u32, &str)] = &[
    (168, "never written: the numbering skipped it"),
    (
        271,
        "taken by issue 330's parked branch (the HTTP transport), salvaged to issue 383",
    ),
];

fn repo() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn decisions_dir() -> PathBuf {
    repo().join("sdlc").join("decisions")
}

/// The number a decision file's name gives, or why the name is refused.
fn number_from_name(name: &str) -> Result<u32, String> {
    let pattern = Regex::new(&format!(r"^({NUMBER})-[a-z0-9-]{{1,50}}\.md$")).unwrap();
    let captures = pattern.captures(name).ok_or_else(|| {
        format!("{name}: the name is not NNNN-<slug>.md (slug [a-z0-9-], at most 50 characters)")
    })?;
    Ok(captures[1].parse().unwrap())
}

/// The number on a decision's first line (`302. **Title (...):** text`), with LF or CRLF.
fn number_from_first_line(text: &str) -> Option<u32> {
    let first = text.lines().next()?.trim_end_matches('\r');
    let (number, rest) = first.split_once(". ")?;
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) || rest.is_empty() {
        return None;
    }
    number.parse().ok()
}

/// Checks a set of decision files (name, text). Returns the numbers they hold, or every problem.
fn check_files(files: &[(String, String)]) -> Result<BTreeMap<u32, String>, Vec<String>> {
    let mut problems = Vec::new();
    let mut numbers: BTreeMap<u32, String> = BTreeMap::new();
    for (name, text) in files {
        let from_name = match number_from_name(name) {
            Ok(n) => n,
            Err(problem) => {
                problems.push(problem);
                continue;
            }
        };
        match number_from_first_line(text) {
            Some(n) if n == from_name => {}
            Some(n) => problems.push(format!(
                "{name}: the name says {from_name} but the first line says {n}"
            )),
            None => problems.push(format!(
                "{name}: the first line is not `{from_name}. **Title:** text`"
            )),
        }
        if let Some(other) = numbers.insert(from_name, name.clone()) {
            problems.push(format!(
                "{name}: number {from_name} is also used by {other}"
            ));
        }
    }
    if problems.is_empty() {
        Ok(numbers)
    } else {
        Err(problems)
    }
}

// A decision number as file names write it (4 digits) and as references write it (1 to 4).
const NUMBER: &str = r"\d{4}";
const REFERENCE_NUMBER: &str = r"\d{1,4}";

// A separator between list items; "and"/"or" come first so ", and " isn't read as ",".
const SEPARATOR_PATTERN: &str = r",?\s+(?:and|or)\s+|,\s*";

static BRACKET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"\[({REFERENCE_NUMBER})\]")).unwrap());
static WORDS: LazyLock<Regex> = LazyLock::new(|| {
    let item = format!(r"{REFERENCE_NUMBER}\b(?:\s*[-–]\s*{REFERENCE_NUMBER}\b)?");
    Regex::new(&format!(
        r"\b[Dd]ecisions? ({item}(?:(?:{SEPARATOR_PATTERN}){item})*)"
    ))
    .unwrap()
});
static SEPARATOR: LazyLock<Regex> = LazyLock::new(|| Regex::new(SEPARATOR_PATTERN).unwrap());
// A date (`2026-10-22`) or a month (`2026-10`): blanked out before word references are read.
static DATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b\d{4}-\d{2}(?:-\d{2})?\b").unwrap());
static RANGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d+)\s*[-–]\s*(\d+)$").unwrap());

/// Every decision number a line refers to: `[n]` (only counted in `sdlc/`, and not right after a
/// word character, so `contestants[0]` is no reference), and "decision N", "decisions N, M",
/// "decisions N and M", "decisions N-M" anywhere. A range counts every number in it. Dates and
/// months are blanked out first, so a date after a reference (`decision 241, 2026-10-22`) is not
/// read as one.
fn references_in_line(line: &str, brackets: bool) -> Vec<u32> {
    let mut numbers = Vec::new();
    if brackets {
        for captures in BRACKET.captures_iter(line) {
            let start = captures.get(0).unwrap().start();
            let before = line[..start].chars().next_back();
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            numbers.push(captures[1].parse().unwrap());
        }
    }
    let without_dates = DATE.replace_all(line, |c: &regex::Captures| " ".repeat(c[0].len()));
    for captures in WORDS.captures_iter(&without_dates) {
        for part in SEPARATOR.split(&captures[1]) {
            match RANGE.captures(part) {
                Some(r) => {
                    let (from, to): (u32, u32) = (r[1].parse().unwrap(), r[2].parse().unwrap());
                    numbers.extend(from..=to);
                }
                None => numbers.push(part.parse().unwrap()),
            }
        }
    }
    numbers
}

/// Every reference in one file that resolves to no decision file and is not allow-listed, as
/// `path:line: decision N has no file`.
fn unresolved_references(path: &str, text: &str, known: &BTreeMap<u32, String>) -> Vec<String> {
    let brackets = path.starts_with("sdlc/");
    let mut problems = Vec::new();
    for (index, line) in text.lines().enumerate() {
        for number in references_in_line(line, brackets) {
            if !known.contains_key(&number) && !UNWRITTEN.iter().any(|(n, _)| *n == number) {
                problems.push(format!(
                    "{path}:{}: decision {number} has no file in sdlc/decisions/",
                    index + 1
                ));
            }
        }
    }
    problems
}

fn repo_decision_files() -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = std::fs::read_dir(decisions_dir())
        .expect("sdlc/decisions/ exists")
        .map(|entry| {
            let entry = entry.unwrap();
            let name = entry.file_name().into_string().unwrap();
            let text = std::fs::read_to_string(entry.path()).unwrap();
            (name, text)
        })
        .collect();
    files.sort();
    files
}

fn tracked_md_and_rs_files(root: &Path) -> Vec<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z", "--", "*.md", "*.rs"])
        .output()
        .expect("git runs");
    assert!(output.status.success(), "git ls-files failed");
    String::from_utf8(output.stdout)
        .unwrap()
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect()
}

/// The word a reference starts with. The tests build their deliberately missing references from
/// it, so this file holds no literal reference that the repo scan would read as a real one.
const WORD: &str = "decision";

fn named(files: &[(&str, &str)]) -> Vec<(String, String)> {
    files
        .iter()
        .map(|(name, text)| (name.to_string(), text.to_string()))
        .collect()
}

// --- The repo's own decisions ---

#[test]
fn every_decision_file_in_the_repo_is_valid_and_numbered_once() {
    let files = repo_decision_files();
    let numbers = check_files(&files).unwrap_or_else(|problems| panic!("{problems:#?}"));
    // The move kept decisions 1 to 301 apart from the two unwritten numbers.
    for n in 1..=301 {
        let unwritten = UNWRITTEN.iter().any(|(u, _)| *u == n);
        assert_eq!(numbers.contains_key(&n), !unwritten, "decision {n}");
    }
}

#[test]
fn allow_listed_numbers_really_have_no_file() {
    let numbers = check_files(&repo_decision_files()).unwrap();
    for (n, reason) in UNWRITTEN {
        assert!(
            !numbers.contains_key(n),
            "{n} has a file now; drop it from UNWRITTEN ({reason})"
        );
    }
}

/// Every unresolved reference in the tracked `.md` and `.rs` files of the git checkout at `root`.
fn reference_problems(root: &Path, known: &BTreeMap<u32, String>) -> Vec<String> {
    let mut problems = Vec::new();
    for path in tracked_md_and_rs_files(root) {
        let Ok(text) = std::fs::read_to_string(root.join(&path)) else {
            continue; // deleted in the working tree, or not UTF-8
        };
        problems.extend(unresolved_references(&path, &text, known));
    }
    problems
}

#[test]
fn every_reference_to_a_decision_in_tracked_files_resolves_to_a_file() {
    let numbers = check_files(&repo_decision_files()).unwrap();
    let problems = reference_problems(repo(), &numbers);
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn decisions_md_keeps_the_rounds_and_spikes_but_no_entries() {
    let text = std::fs::read_to_string(repo().join("sdlc/decisions.md")).unwrap();
    assert!(text.contains("## Round 1 (2026-09-24)"));
    assert!(text.contains("## Open research spikes"));
    assert!(text.contains("## Spike results"));
    assert!(text.contains("`sdlc/decisions/`"));
    let entry = Regex::new(r"^\d+\. \*\*").unwrap();
    let entries: Vec<&str> = text.lines().filter(|line| entry.is_match(line)).collect();
    assert!(entries.is_empty(), "{entries:#?}");
}

// --- File names (boundary cases) ---

#[test]
fn file_names_from_1_to_9999_with_a_lowercase_slug_are_accepted() {
    assert_eq!(number_from_name("0001-sandbox-contents.md"), Ok(1));
    assert_eq!(number_from_name("0301-the-highest.md"), Ok(301));
    assert_eq!(number_from_name("9999-x.md"), Ok(9999));
    assert_eq!(number_from_name("0042-a-2-b.md"), Ok(42));
    let slug_of_50 = "a".repeat(50);
    assert_eq!(number_from_name(&format!("0007-{slug_of_50}.md")), Ok(7));
}

#[test]
fn file_names_with_the_wrong_digits_case_or_slug_length_are_refused() {
    let slug_of_51 = "a".repeat(51);
    for name in [
        "302-three-digits.md".to_string(),
        "00302-five-digits.md".to_string(),
        "0302-Uppercase.md".to_string(),
        format!("0302-{slug_of_51}.md"),
        "0302-.md".to_string(),
        "0302-under_score.md".to_string(),
        "0302-slug.txt".to_string(),
        "0302-slug.md.bak".to_string(),
    ] {
        let problem = number_from_name(&name).unwrap_err();
        assert!(problem.starts_with(&name), "{problem}");
    }
}

// --- File contents ---

#[test]
fn a_name_whose_number_differs_from_the_first_line_is_refused() {
    let problems =
        check_files(&named(&[("0005-x.md", "6. **X (2026-09-24):** text\n")])).unwrap_err();
    assert_eq!(
        problems,
        ["0005-x.md: the name says 5 but the first line says 6"]
    );
}

#[test]
fn a_first_line_with_no_number_is_refused() {
    let problems = check_files(&named(&[("0005-x.md", "**X (2026-09-24):** text\n")])).unwrap_err();
    assert_eq!(problems.len(), 1);
    assert!(
        problems[0].starts_with("0005-x.md: the first line"),
        "{problems:?}"
    );
}

#[test]
fn two_files_with_the_same_number_are_refused() {
    let problems = check_files(&named(&[
        ("0005-first.md", "5. **First:** text\n"),
        ("0005-second.md", "5. **Second:** text\n"),
    ]))
    .unwrap_err();
    assert_eq!(
        problems,
        ["0005-second.md: number 5 is also used by 0005-first.md"]
    );
}

#[test]
fn gaps_in_the_numbers_are_allowed() {
    let numbers = check_files(&named(&[
        ("0001-one.md", "1. **One:** text\n"),
        ("0003-three.md", "3. **Three:** text\n"),
        ("0303-later.md", "303. **Later:** text\n"),
    ]))
    .unwrap();
    assert_eq!(numbers.keys().copied().collect::<Vec<_>>(), [1, 3, 303]);
}

#[test]
fn crlf_and_lf_files_both_pass() {
    let numbers = check_files(&named(&[
        ("0001-lf.md", "1. **Lf:** one\n    more\n"),
        ("0002-crlf.md", "2. **Crlf:** one\r\n    more\r\n"),
        ("0003-crlf-one-line.md", "3. **One line:** text\r\n"),
    ]))
    .unwrap();
    assert_eq!(numbers.len(), 3);
}

// --- References ---

#[test]
fn references_are_read_in_every_written_form() {
    assert_eq!(references_in_line("as decision 56 says", false), [56]);
    assert_eq!(references_in_line("Decision 7.", false), [7]);
    assert_eq!(references_in_line("decisions 176, 179", false), [176, 179]);
    assert_eq!(
        references_in_line("decisions 182 and 184", false),
        [182, 184]
    );
    assert_eq!(
        references_in_line("decisions 65, 208, and 230", false),
        [65, 208, 230]
    );
    assert_eq!(
        references_in_line("decisions 80-82, 134, and 139-141", false),
        [80, 81, 82, 134, 139, 140, 141]
    );
    assert_eq!(references_in_line("decision 5 or 6", false), [5, 6]);
    assert_eq!(references_in_line("decisions 59–61", false), [59, 60, 61]);
    assert_eq!(
        references_in_line("(decisions 252-254, 266 and 280)", false),
        [252, 253, 254, 266, 280]
    );
    assert_eq!(
        references_in_line("follows [68] and [88][90]", true),
        [68, 88, 90]
    );
}

#[test]
fn brackets_outside_sdlc_dates_and_indexing_are_not_references() {
    assert!(references_in_line("follows [68]", false).is_empty());
    assert!(references_in_line("contestants[0] (claude)", true).is_empty());
    assert_eq!(
        references_in_line("(decision 241, 2026-10-22)", false),
        [241]
    );
    assert!(references_in_line("the user's indecision 5", false).is_empty());
}

#[test]
fn a_reference_to_an_allow_listed_gap_passes() {
    let known = check_files(&named(&[("0001-one.md", "1. **One:** text\n")])).unwrap();
    assert!(unresolved_references("sdlc/x.md", "see [168] and decision 168\n", &known).is_empty());
}

#[test]
fn a_reference_to_a_number_with_no_file_names_the_file_and_line() {
    let known = check_files(&named(&[("0001-one.md", "1. **One:** text\n")])).unwrap();
    let text = "decision 1 is fine\r\nbut decisions 1-2 are not\r\n";
    assert_eq!(
        unresolved_references("docs/x.md", text, &known),
        ["docs/x.md:2: decision 2 has no file in sdlc/decisions/"]
    );
    assert_eq!(
        unresolved_references("sdlc/y.md", "[1]\n[9]\n", &known),
        ["sdlc/y.md:2: decision 9 has no file in sdlc/decisions/"]
    );
}

// Review M-1: four-digit numbers are read like any other, as the file names allow them.
#[test]
fn four_digit_references_are_checked_like_any_other() {
    let none = BTreeMap::new();
    let missing = |path: &str, line: usize, n: u32| {
        format!("{path}:{line}: {WORD} {n} has no file in sdlc/decisions/")
    };
    assert_eq!(
        unresolved_references("docs/probe.md", &format!("x\n{WORD} 1000\n"), &none),
        [missing("docs/probe.md", 2, 1000)]
    );
    for line in [
        format!("{WORD}s 9998, 9999"),
        format!("{WORD}s 9998-9999"),
        "[9998][9999]".to_string(),
    ] {
        assert_eq!(
            unresolved_references("sdlc/probe.md", &line, &none),
            [
                missing("sdlc/probe.md", 1, 9998),
                missing("sdlc/probe.md", 1, 9999)
            ],
            "{line}"
        );
    }
    let known = check_files(&named(&[
        ("1000-a.md", "1000. **A:** text\n"),
        ("9998-b.md", "9998. **B:** text\n"),
        ("9999-c.md", "9999. **C:** text\n"),
    ]))
    .unwrap();
    let text = format!("{WORD} 1000\n{WORD}s 9998, 9999\n{WORD}s 9998-9999\n");
    assert!(unresolved_references("docs/probe.md", &text, &known).is_empty());
}

#[test]
fn a_date_right_after_a_reference_is_still_no_reference() {
    assert_eq!(
        references_in_line("(decision 241, 2026-10-22)", false),
        [241]
    );
    assert_eq!(
        references_in_line("decisions 12 and 2026-10-22", false),
        [12]
    );
    assert_eq!(references_in_line("decision 7, 2026-10", false), [7]);
    assert!(references_in_line("decision 2026-10-22", false).is_empty());
}

// Review M-2: the scan reads every tracked file, this test file included.
#[test]
fn the_repo_scan_reads_every_tracked_file_this_test_file_included() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::create_dir_all(dir.join("tests")).unwrap();
    std::fs::create_dir_all(dir.join("docs")).unwrap();
    let missing = format!("// This follows {WORD} 1000.\n");
    std::fs::write(dir.join("tests/decisions.rs"), &missing).unwrap();
    std::fs::write(dir.join("docs/a.md"), format!("x\n{missing}")).unwrap();
    std::fs::write(dir.join("docs/untracked.md"), &missing).unwrap();
    git(dir, &["init", "-q"]);
    git(dir, &["add", "tests/decisions.rs", "docs/a.md"]);

    let mut problems = reference_problems(dir, &BTreeMap::new());
    problems.sort();

    assert_eq!(
        problems,
        [
            format!("docs/a.md:2: {WORD} 1000 has no file in sdlc/decisions/"),
            format!("tests/decisions.rs:1: {WORD} 1000 has no file in sdlc/decisions/"),
        ]
    );
}

// --- Two branches adding a decision each ---

fn git(dir: &Path, args: &[&str]) -> std::process::Output {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=sbxm-test",
            "-c",
            "user.email=sbxm-test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.autocrlf=false",
        ])
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn two_branches_that_each_add_a_decision_file_merge_without_a_conflict() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let folder = dir.join("sdlc").join("decisions");
    std::fs::create_dir_all(&folder).unwrap();
    git(dir, &["init", "-q"]);
    std::fs::write(folder.join("0301-base.md"), "301. **Base:** text\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "base"]);
    git(dir, &["branch", "-M", "main"]);

    git(dir, &["checkout", "-q", "-b", "one"]);
    std::fs::write(folder.join("0302-one.md"), "302. **One:** text\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "one"]);

    git(dir, &["checkout", "-q", "-b", "two", "main"]);
    std::fs::write(folder.join("0303-two.md"), "303. **Two:** text\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "two"]);

    git(dir, &["checkout", "-q", "main"]);
    git(dir, &["merge", "-q", "--no-edit", "one"]);
    git(dir, &["merge", "-q", "--no-edit", "two"]);

    let mut files: Vec<(String, String)> = std::fs::read_dir(&folder)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().into_string().unwrap(),
                std::fs::read_to_string(entry.path()).unwrap(),
            )
        })
        .collect();
    files.sort();
    let numbers = check_files(&files).unwrap();
    assert_eq!(numbers.keys().copied().collect::<Vec<_>>(), [301, 302, 303]);
}
