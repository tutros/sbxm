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

/// `line` without its `//` comment and without the text of its string literals (a naive cut:
/// enough for import paths, which never sit inside a string).
fn code_of(line: &str) -> String {
    let mut code = String::new();
    let mut in_string = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if in_string => {
                chars.next();
            }
            '"' => {
                in_string = !in_string;
                code.push('"');
            }
            '/' if !in_string && chars.peek() == Some(&'/') => break,
            _ if in_string => {}
            _ => code.push(c),
        }
    }
    code
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
    let code: Vec<String> = text.lines().map(code_of).collect();
    let mut found = Vec::new();
    for (i, line) in code.iter().enumerate() {
        let squeezed: String = line.split_whitespace().collect();
        if squeezed.contains("crate::task") {
            found.push(i + 1);
            continue;
        }
        if let Some(at) = squeezed.find("crate::{") {
            let mut group = squeezed[at + "crate::{".len()..].to_owned();
            let mut next = i + 1;
            while group.matches('{').count() >= group.matches('}').count() && next < code.len() {
                group.extend(code[next].split_whitespace());
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
                use crate::shared::task_like;\n";

    assert!(naming_task(text).is_empty(), "{:?}", naming_task(text));
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
