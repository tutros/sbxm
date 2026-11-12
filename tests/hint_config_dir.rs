//! Issue 589: the context block's `SBXM_CONFIG_DIR` line selects the same config as the failed
//! command. A relative value is printed as the effective absolute dir when the block's `cd` goes
//! elsewhere, and as given when it resolves to the same directory either way.

mod common;

use std::ffi::OsString;
use std::path::PathBuf;
#[cfg(unix)]
use std::{path::Path, process::Command};

#[cfg(unix)]
use common::Env;
use sbxm::hint::{self, Context, Shell};

const HEADER: &str = "to run the commands above as this one ran, first:\n";

/// A block context: the `cd` goes to `dir`, the command ran in `cwd`.
fn context(dir: &str, cwd: &str, config_dir: Option<&str>) -> Context {
    Context {
        dir: Some(PathBuf::from(dir)),
        cwd: Some(PathBuf::from(cwd)),
        exe: Some(PathBuf::from("/opt/sbxm")),
        config_dir: config_dir.map(OsString::from),
    }
}

/// The config line of `context`'s block in `shell`, `None` when it has none.
fn config_line(context: &Context, shell: Shell) -> Option<String> {
    hint::block(context, shell)
        .lines()
        .map(str::trim_start)
        .find(|l| l.contains("SBXM_CONFIG_DIR"))
        .map(str::to_owned)
}

/// An absolute path of this platform, for the string tests.
fn abs(unix: &str) -> String {
    if cfg!(windows) {
        format!("H:{}", unix.replace('/', "\\"))
    } else {
        unix.to_owned()
    }
}

#[test]
fn an_absolute_value_is_printed_as_given() {
    let given = abs("/cfg/sbxm");
    let ctx = context(&abs("/repo"), &abs("/elsewhere"), Some(&given));
    for shell in [Shell::Sh, Shell::PowerShell] {
        assert_eq!(config_line(&ctx, shell), Some(shell.set_config_dir(&given)));
    }
}

#[test]
fn a_relative_value_is_printed_absolute_when_the_block_cds_elsewhere() {
    let ctx = context(&abs("/repo"), &abs("/start here"), Some("./cfg dir"));
    let expected = PathBuf::from(abs("/start here")).join("cfg dir");
    for shell in [Shell::Sh, Shell::PowerShell] {
        assert_eq!(
            config_line(&ctx, shell),
            Some(shell.set_config_dir(&expected.to_string_lossy())),
            "{shell:?}"
        );
    }
}

#[test]
fn a_relative_value_with_dot_dot_keeps_it_in_the_absolute_form() {
    // `..` is left to the OS: dropping it with the segment before it would be wrong after a link.
    let ctx = context(&abs("/repo"), &abs("/start"), Some("../cfg"));
    let expected = PathBuf::from(abs("/start")).join("..").join("cfg");
    assert_eq!(
        config_line(&ctx, Shell::HOST),
        Some(Shell::HOST.set_config_dir(&expected.to_string_lossy()))
    );
}

#[test]
fn a_relative_value_is_printed_as_given_when_the_cd_is_the_working_directory() {
    let ctx = context(&abs("/repo"), &abs("/repo"), Some("./cfg"));
    for shell in [Shell::Sh, Shell::PowerShell] {
        assert_eq!(
            config_line(&ctx, shell),
            Some(shell.set_config_dir("./cfg"))
        );
    }
}

#[test]
fn a_relative_value_is_printed_as_given_when_there_is_no_cd() {
    let ctx = Context {
        dir: None,
        ..context(&abs("/repo"), &abs("/start"), Some("./cfg"))
    };
    assert_eq!(
        config_line(&ctx, Shell::HOST),
        Some(Shell::HOST.set_config_dir("./cfg"))
    );
}

#[test]
fn an_unset_value_prints_no_config_line() {
    let ctx = context(&abs("/repo"), &abs("/start"), None);
    assert_eq!(config_line(&ctx, Shell::HOST), None);
    assert_eq!(
        hint::block(&ctx, Shell::Sh),
        format!("{HEADER}  {}\n", Shell::Sh.cd(&abs("/repo")))
    );
}

#[test]
fn the_current_context_records_the_working_directory() {
    assert_eq!(Context::current().cwd, std::env::current_dir().ok());
}

/// `start/link` is a link to `real`, so `link/../cfg` is `cfg` beside `real` (where the OS
/// resolves it), not `start/cfg` (where dropping `link/..` would put it). Each `cfg` holds a
/// `config.toml` naming itself.
#[cfg(unix)]
struct Tree {
    _tmp: tempfile::TempDir,
    start: PathBuf,
    repo: PathBuf,
}

#[cfg(unix)]
fn tree() -> Tree {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let start = root.join("start 'q");
    let repo = root.join("the [repo]");
    for dir in [&start, &repo, &root.join("real"), &root.join("cfg")] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::os::unix::fs::symlink(root.join("real"), start.join("link")).unwrap();
    std::fs::create_dir_all(start.join("cfg")).unwrap();
    std::fs::write(root.join("cfg/config.toml"), "beside real\n").unwrap();
    std::fs::write(start.join("cfg/config.toml"), "in start\n").unwrap();
    Tree {
        _tmp: tmp,
        start,
        repo,
    }
}

/// Runs `context`'s block lines in `sh` from `cwd`, then prints the config they select.
#[cfg(unix)]
fn run_block(context: &Context, cwd: &Path) -> String {
    let mut script: String = hint::block(context, Shell::Sh)
        .lines()
        .skip(1)
        .map(|l| format!("{}\n", l.trim_start()))
        .collect();
    script.push_str("cat \"$SBXM_CONFIG_DIR/config.toml\"\n");
    let output = Command::new("sh")
        .arg("-c")
        .arg(&script)
        .current_dir(cwd)
        .env_remove("SBXM_CONFIG_DIR")
        .output()
        .unwrap();
    assert!(output.status.success(), "{script}\n{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

#[cfg(unix)]
#[test]
fn the_printed_lines_select_the_same_config_through_dot_dot_and_a_link() {
    let t = tree();
    let value = "link/../cfg";
    let ctx = Context {
        dir: Some(t.repo.clone()),
        cwd: Some(t.start.clone()),
        exe: None,
        config_dir: Some(value.into()),
    };
    let printed = config_line(&ctx, Shell::Sh).unwrap();
    assert!(!printed.contains(&Shell::Sh.quote(value)), "{printed}");
    let selected = std::fs::canonicalize(t.start.join(value)).unwrap();
    assert_eq!(
        std::fs::read_to_string(selected.join("config.toml")).unwrap(),
        "beside real\n"
    );
    assert_eq!(run_block(&ctx, &t.start), "beside real\n");
}

#[cfg(unix)]
#[test]
fn the_printed_lines_select_the_same_config_when_the_cd_is_the_working_directory() {
    let t = tree();
    let ctx = Context {
        dir: Some(t.start.clone()),
        cwd: Some(t.start.clone()),
        exe: None,
        config_dir: Some("./cfg".into()),
    };
    assert_eq!(
        config_line(&ctx, Shell::Sh),
        Some(Shell::Sh.set_config_dir("./cfg"))
    );
    assert_eq!(run_block(&ctx, &t.repo), "in start\n");
}

/// The review's reproduction (PR #588, Q-1) through the binary: a task command run with a relative
/// `SBXM_CONFIG_DIR` away from the task's checkout prints a block whose lines, run as printed,
/// load the same config (here `config profiles-dir` reads it), not `no config at ../config`.
#[cfg(unix)]
#[test]
fn the_binarys_block_loads_the_same_config_after_its_cd() {
    use sbxm::task::record::{self, Kind, NewTask, Process};
    let env = Env::new();
    let root = std::fs::canonicalize(env.tmp.path()).unwrap();
    let checkout = root.join("the repo");
    let here = root.join("elsewhere");
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::create_dir_all(&here).unwrap();
    let mut task = record::Record::new(
        &NewTask {
            kind: Kind::Issue,
            number: 41,
            repo: "o/r",
            title: "t",
            base: "main",
            branch: "b",
            config_hash: "h",
            id: None,
        },
        1_790_000_000,
        Process::new(777_777, 1_790_000_000),
    );
    task.checkout = Some(checkout.to_string_lossy().into_owned());
    record::write(&record::task_dir(&env.base_dir(), &task.id), &task).unwrap();

    let exe = assert_cmd::cargo::cargo_bin("sbxm");
    let output = Command::new(&exe)
        .current_dir(&here)
        .env("SBXM_CONFIG_DIR", "../config")
        .args(["task", "gates", "--issue", "41"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    let block = &stderr[stderr.find(HEADER).expect(&stderr)..];
    assert!(
        block.contains(&Shell::Sh.cd(&checkout.to_string_lossy())),
        "{block}"
    );
    assert!(
        block.contains(&Shell::Sh.set_config_dir(&here.join("../config").to_string_lossy())),
        "{block}"
    );

    let mut script: String = block
        .lines()
        .skip(1)
        .map(|l| format!("{}\n", l.trim_start()))
        .collect();
    script.push_str(&format!(
        "{} config profiles-dir\n",
        Shell::Sh.quote(&exe.to_string_lossy())
    ));
    let output = Command::new("sh")
        .arg("-c")
        .arg(&script)
        .current_dir(&here)
        .env_remove("SBXM_CONFIG_DIR")
        .output()
        .unwrap();
    assert!(output.status.success(), "{script}\n{output:?}");
    assert_eq!(
        std::fs::canonicalize(String::from_utf8(output.stdout).unwrap().trim()).unwrap(),
        std::fs::canonicalize(env.profiles_dir()).unwrap()
    );
}
