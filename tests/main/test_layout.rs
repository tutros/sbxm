//! Issue 473: the integration tests are one test binary, `tests/main/main.rs`, with one module per
//! file. A file in `tests/main/` that `main.rs` doesn't name is never compiled, so its tests would
//! be lost without a word; a new file directly in `tests/` would be a test binary of its own.

use std::path::{Path, PathBuf};

/// The files directly in `tests/` that are test binaries of their own: the real-`sbx` tests, run
/// by hand with `--ignored`, and the tests that set state for their whole process.
const OWN_BINARIES: [&str; 7] = [
    "hint.rs",
    "hint_viewers.rs",
    "real_jev.rs",
    "real_sbx.rs",
    "real_sbx_cloud.rs",
    "task_loop_owner_stamp.rs",
    "task_start_unlabel_loop.rs",
];

fn tests_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

/// The names of the `.rs` files directly in `dir`.
fn rust_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "rs"))
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn every_file_in_tests_main_is_a_module_of_main_rs() {
    let dir = tests_dir().join("main");
    let main = std::fs::read_to_string(dir.join("main.rs")).unwrap();
    let declared: Vec<&str> = main.lines().map(str::trim).collect();

    let missing: Vec<String> = rust_files(&dir)
        .into_iter()
        .filter(|name| name != "main.rs")
        .filter(|name| {
            let line = format!("mod {};", name.trim_end_matches(".rs"));
            !declared.contains(&line.as_str())
        })
        .collect();

    assert!(
        missing.is_empty(),
        "add a `mod <name>;` line to tests/main/main.rs for: {missing:?}"
    );
}

#[test]
fn the_only_test_binaries_beside_main_are_the_known_ones() {
    let own: Vec<String> = rust_files(&tests_dir());

    assert_eq!(
        own, OWN_BINARIES,
        "a new test file goes in tests/main/ with a `mod` line in tests/main/main.rs"
    );
}
