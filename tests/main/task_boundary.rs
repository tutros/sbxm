//! Issue 539: the dependency between `sbxm task` and the sandbox side runs one way, task ->
//! sandbox. No sandbox-side source file names `crate::task`; what both sides need lives in
//! `src/shared/`, and the task side (or a top-level command) hands in anything else.

use std::path::{Path, PathBuf};

/// The sandbox side: these folders (every `.rs` file under them) and files, from `src/`.
const SANDBOX_DIRS: [&str; 4] = ["backend", "run", "eval", "github"];
const SANDBOX_FILES: [&str; 5] = [
    "slot.rs",
    "template.rs",
    "commands/template_build.rs",
    "commands/run.rs",
    "commands/run_review.rs",
];

fn src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn rust_files_under(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files_under(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}

fn sandbox_side() -> Vec<PathBuf> {
    let src = src();
    let mut files = Vec::new();
    for dir in SANDBOX_DIRS {
        rust_files_under(&src.join(dir), &mut files);
    }
    for file in SANDBOX_FILES {
        files.push(
            file.split('/')
                .fold(src.clone(), |path, part| path.join(part)),
        );
    }
    files.sort();
    files
}

/// The code of each line of `text`. The whole file is read as one, so a comment (`//`, a nested
/// `/* */`) or a literal (a string, raw string or char, over any number of lines) leaves only its
/// line breaks; a literal becomes `""` where it starts.
fn code_lines(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut code = String::new();
    let mut i = 0;
    while i < chars.len() {
        match skipped(&chars, i) {
            Some((end, literal)) => {
                if literal {
                    code.push_str("\"\"");
                }
                code.extend(chars[i..end].iter().filter(|c| **c == '\n'));
                i = end;
            }
            None => {
                code.push(chars[i]);
                i += 1;
            }
        }
    }
    code.split('\n').map(str::to_owned).collect()
}

/// For a comment or literal starting at `at`: the index past its end, and whether it is a
/// literal. `None` for anything else (a lifetime's `'` included).
fn skipped(chars: &[char], at: usize) -> Option<(usize, bool)> {
    let get = |i: usize| chars.get(i).copied();
    let after_ident = at > 0 && (chars[at - 1].is_alphanumeric() || chars[at - 1] == '_');
    let find = |from: usize, close: &dyn Fn(usize) -> bool| {
        (from..chars.len())
            .find(|i| close(*i))
            .unwrap_or(chars.len())
    };
    match (chars[at], get(at + 1)) {
        ('/', Some('/')) => Some((find(at, &|i| chars[i] == '\n'), false)),
        ('/', Some('*')) => Some((block_comment_end(chars, at), false)),
        ('"', _) => {
            let close = find(at + 1, &|i| chars[i] == '"' && !escaped(chars, i));
            Some(((close + 1).min(chars.len()), true))
        }
        ('r', Some('"' | '#')) if !after_ident => {
            let hashes = chars[at + 1..].iter().take_while(|c| **c == '#').count();
            if get(at + 1 + hashes) != Some('"') {
                return None;
            }
            let close = find(at + 2 + hashes, &|i| {
                chars[i] == '"' && (1..=hashes).all(|h| get(i + h) == Some('#'))
            });
            Some(((close + 1 + hashes).min(chars.len()), true))
        }
        ('\'', Some('\\')) => {
            let close = find(at + 3, &|i| chars[i] == '\'');
            Some(((close + 1).min(chars.len()), true))
        }
        ('\'', Some(_)) if get(at + 2) == Some('\'') => Some((at + 3, true)),
        _ => None,
    }
}

/// Whether the char at `at` follows an odd run of backslashes (an escaped quote).
fn escaped(chars: &[char], at: usize) -> bool {
    chars[..at].iter().rev().take_while(|c| **c == '\\').count() % 2 == 1
}

/// The index past the `*/` closing the block comment that opens at `at`, nested ones counted.
fn block_comment_end(chars: &[char], at: usize) -> usize {
    let mut depth = 0;
    let mut i = at;
    while i + 1 < chars.len() {
        match (chars[i], chars[i + 1]) {
            ('/', '*') => {
                depth += 1;
                i += 2;
            }
            ('*', '/') => {
                depth -= 1;
                i += 2;
                if depth == 0 {
                    return i;
                }
            }
            _ => i += 1,
        }
    }
    chars.len()
}

/// `line` with each run of whitespace made one space, and none around `::`, `{`, `}` and `,`,
/// so `crate :: task` reads as `crate::task` while `task as tasks` keeps its words apart.
fn normalized(line: &str) -> String {
    let spaced = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::new();
    for part in spaced.split(' ') {
        let joins = |c: Option<char>| matches!(c, Some(':' | '{' | '}' | ','));
        if !out.is_empty() && !joins(out.chars().last()) && !joins(part.chars().next()) {
            out.push(' ');
        }
        out.push_str(part);
    }
    out
}

/// Whether `line` has the path `crate::task` itself, not a longer name (`crate::tasks`).
fn names_task_path(line: &str) -> bool {
    line.match_indices("crate::task").any(|(at, path)| {
        line[at + path.len()..]
            .chars()
            .next()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
    })
}

/// Whether `group`, the inside of a `crate::{...}` import, names `task` at its top.
fn group_names_task(group: &str) -> bool {
    let mut depth = 0;
    let mut item = String::new();
    let mut items = Vec::new();
    for c in group.chars() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => items.push(std::mem::take(&mut item)),
            _ if depth == 0 => item.push(c),
            _ => {}
        }
    }
    items.push(item);
    items.iter().any(|item| {
        let item = item.trim();
        item == "task" || item.starts_with("task::") || item.starts_with("task as ")
    })
}

/// The lines (1-based) of `text` whose code names `crate::task`, directly or in a
/// `use crate::{.., task, ..}` group.
fn naming_task(text: &str) -> Vec<usize> {
    let code: Vec<String> = code_lines(text).iter().map(|l| normalized(l)).collect();
    let mut found = Vec::new();
    for (i, line) in code.iter().enumerate() {
        if names_task_path(line) {
            found.push(i + 1);
            continue;
        }
        if let Some(at) = line.find("crate::{") {
            let mut group = line[at + "crate::{".len()..].to_owned();
            let mut next = i + 1;
            while group.matches('{').count() >= group.matches('}').count() && next < code.len() {
                group.push(' ');
                group.push_str(&code[next]);
                next += 1;
            }
            if let Some(end) = closing(&group) {
                group.truncate(end);
            }
            if group_names_task(&group) {
                found.push(i + 1);
            }
        }
    }
    found
}

/// Where the group's own closing `}` is in `group` (the text after its opening `{`).
fn closing(group: &str) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in group.char_indices() {
        match c {
            '{' => depth += 1,
            '}' if depth == 0 => return Some(i),
            '}' => depth -= 1,
            _ => {}
        }
    }
    None
}

#[test]
fn task_boundary_no_sandbox_side_file_names_crate_task() {
    let src = src();
    let offending: Vec<String> = sandbox_side()
        .iter()
        .flat_map(|path| {
            let text = std::fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
            let shown = path.strip_prefix(&src).unwrap().display().to_string();
            naming_task(&text)
                .into_iter()
                .map(move |line| format!("{shown}:{line}"))
        })
        .collect();

    assert!(
        offending.is_empty(),
        "the sandbox side must not name crate::task (issue 539); move the helper to \
         src/shared/ or hand the data in: {offending:?}"
    );
}

#[test]
fn task_boundary_lists_existing_files() {
    for path in sandbox_side() {
        assert!(path.is_file(), "{} is not a file", path.display());
    }
}

#[test]
fn task_boundary_flags_a_use_and_a_path() {
    let text =
        "use std::fs;\nuse crate::task::record::Kind;\nfn f() { crate::task::lock::token(); }\n";

    assert_eq!(naming_task(text), [2, 3]);
}

#[test]
fn task_boundary_flags_task_in_a_crate_group() {
    let text =
        "use crate::{disk, kit, seed, task};\nuse crate::{\n    run,\n    task::record,\n};\n";

    assert_eq!(naming_task(text), [1, 2]);
}

#[test]
fn task_boundary_ignores_comments_strings_and_other_paths() {
    let text = "//! Like `crate::task::runlog`, part 3.\n\
                /// See [`crate::task::lock`].\n\
                let x = 1; // crate::task::record\n\
                let s = \"crate::task::x\";\n\
                use crate::{tasks, retask, run::task};\n\
                use crate::shared::task_like;\n\
                use crate::tasks::x;\n";

    assert!(naming_task(text).is_empty(), "{:?}", naming_task(text));
}

#[test]
fn task_boundary_flags_a_task_alias_in_a_crate_group() {
    let text = r"use crate::{config, task as tasks};
use crate::{
    config,
    task as tasks,
};
";

    assert_eq!(naming_task(text), [1, 2]);
}

#[test]
fn task_boundary_ignores_strings_and_comments_over_several_lines() {
    let text = r###"let message = "help:
crate::task::record
";
let raw = r#"a "quoted" crate::task::x
crate::task::y "#;
/* a block comment
   crate::task::z */
let quote = '"';
let lifetime: &'static str = "crate::task::w";
"###;

    assert!(naming_task(text).is_empty(), "{:?}", naming_task(text));
}

#[test]
fn task_boundary_flags_an_import_after_a_closing_string() {
    let text = r###"let raw = r#"a "quoted"
line"#; use crate::task::record;
let s = "x
"; crate::task::lock::token();
"###;

    assert_eq!(naming_task(text), [2, 4]);
}

#[test]
fn task_boundary_leaves_top_level_files_out() {
    let status = src().join("status").join("tasks.rs");
    let text = std::fs::read_to_string(&status).unwrap();

    assert!(
        !naming_task(&text).is_empty(),
        "status/tasks.rs names crate::task"
    );
    assert!(!sandbox_side().contains(&status));
}
