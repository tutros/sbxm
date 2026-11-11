//! Recovery hints that say where and with what to run them (issue 574): the rewrite of a hint's
//! `sbxm` into the running executable, the context block, and the shell quoting they use.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::CommandFactory;
use sbxm::hint::{self, Context, Local, Shell};

const PROGRAM: &str = "'/opt/my sbxm/sbxm'";

fn rewritten(text: &str) -> Option<String> {
    hint::rewrite(text, PROGRAM)
}

#[test]
fn subcommands_match_the_cli() {
    let mut cli: Vec<String> = sbxm::cli::Cli::command()
        .get_subcommands()
        .map(|c| c.get_name().to_owned())
        .collect();
    let mut known: Vec<String> = hint::subcommands().iter().map(|s| s.to_string()).collect();
    cli.sort();
    known.sort();
    assert_eq!(known, cli);
}

#[test]
fn a_command_at_a_line_start_is_rewritten() {
    assert_eq!(
        rewritten("sbxm task resume --issue 468\n").as_deref(),
        Some("'/opt/my sbxm/sbxm' task resume --issue 468\n")
    );
}

#[test]
fn a_command_after_indentation_is_rewritten() {
    assert_eq!(
        rewritten("hint:\n    sbxm task rm --pr 554 --force\n\tsbxm status\n").as_deref(),
        Some(
            "hint:\n    '/opt/my sbxm/sbxm' task rm --pr 554 --force\n\t'/opt/my sbxm/sbxm' status\n"
        )
    );
}

#[test]
fn a_command_after_next_is_rewritten() {
    assert_eq!(
        rewritten("issue-5: stage ready, must-fix 0; next: sbxm task finish --issue 5").as_deref(),
        Some("issue-5: stage ready, must-fix 0; next: '/opt/my sbxm/sbxm' task finish --issue 5")
    );
}

#[test]
fn a_command_after_a_backtick_is_rewritten_each_time() {
    assert_eq!(
        rewritten("Error: no task issue-9; run `sbxm task status` or `sbxm status`").as_deref(),
        Some(
            "Error: no task issue-9; run `'/opt/my sbxm/sbxm' task status` or \
             `'/opt/my sbxm/sbxm' status`"
        )
    );
}

#[test]
fn sbxm_that_is_not_a_command_stays() {
    for text in [
        "sbxm denied the push",
        "  sbxm-task.toml is missing",
        "cannot read /repo/sbxm-task.toml",
        "see `sbxm-task.toml`",
        "metadata in `.sbxm/demo`",
        "`sbxm-dev task status`",
        "the sbxm task status command",
        "sbxm taskish",
        "next: sbxm-dev task status",
        "",
    ] {
        assert_eq!(rewritten(text), None, "{text:?}");
    }
}

#[test]
fn without_an_executable_the_bare_sbxm_stays() {
    assert_eq!(hint::rewrite_for("sbxm task status", None, Shell::Sh), None);
    assert_eq!(
        hint::rewrite_for("sbxm task status", Some(Path::new("/x/sbxm")), Shell::Sh).as_deref(),
        Some("'/x/sbxm' task status")
    );
}

#[test]
fn powershell_calls_the_quoted_executable() {
    assert_eq!(
        hint::rewrite_for(
            "sbxm task status",
            Some(Path::new(r"H:\sbxm-e2e\sbxm-a4571ff.exe")),
            Shell::PowerShell
        )
        .as_deref(),
        Some(r"& 'H:\sbxm-e2e\sbxm-a4571ff.exe' task status")
    );
}

#[test]
fn each_shell_escapes_a_single_quote_its_own_way() {
    assert_eq!(Shell::Sh.quote("it's"), r"'it'\''s'");
    assert_eq!(Shell::PowerShell.quote("it's"), "'it''s'");
    assert_eq!(
        Shell::PowerShell.quote("it\u{2019}s"),
        "'it\u{2019}\u{2019}s'"
    );
}

fn context(config_dir: Option<&str>) -> Context {
    Context {
        dir: Some(PathBuf::from("/work/my repo")),
        exe: Some(PathBuf::from("/opt/sbxm")),
        config_dir: config_dir.map(OsString::from),
    }
}

#[test]
fn the_block_has_no_config_line_when_sbxm_config_dir_is_unset() {
    let block = hint::block(&context(None), Shell::Sh);
    assert_eq!(
        block,
        "run the sbxm commands above after:\n  cd '/work/my repo'\n"
    );
}

#[test]
fn the_block_sets_sbxm_config_dir_as_given() {
    assert_eq!(
        hint::block(&context(Some("./cfg dir")), Shell::Sh),
        "run the sbxm commands above after:\n  cd '/work/my repo'\n  \
         export SBXM_CONFIG_DIR='./cfg dir'\n"
    );
    assert_eq!(
        hint::block(&context(Some(r"H:\sbxm cfg")), Shell::PowerShell),
        "run the sbxm commands above after:\n  cd '/work/my repo'\n  \
         $env:SBXM_CONFIG_DIR='H:\\sbxm cfg'\n"
    );
}

#[test]
fn the_block_is_empty_with_nothing_to_say() {
    assert_eq!(hint::block(&Context::default(), Shell::Sh), "");
}

#[test]
fn the_writer_rewrites_whole_lines_and_a_partial_one_on_flush() {
    let mut out = Vec::new();
    {
        let mut local = Local::new(&mut out);
        local.write_all(b"run: `sb").unwrap();
        local.write_all(b"xm task status`\nnext: sbxm st").unwrap();
        local.write_all(b"atus").unwrap();
        local.flush().unwrap();
        local.write_all(b"\xff sbxm task\n").unwrap();
    }
    let exe = std::env::current_exe().unwrap();
    let program = Shell::HOST.program(&exe.to_string_lossy());
    let mut expected = format!("run: `{program} task status`\nnext: {program} status").into_bytes();
    expected.extend_from_slice(b"\xff sbxm task\n");
    assert_eq!(out, expected);
}

// The printed lines run as written, with a space and a single quote in the directory, the
// executable and the config dir, in each shell that is on this machine.

/// A temp dir holding `dir with 'quote` and, inside another such dir, an executable that prints
/// its working directory, its arguments and `SBXM_CONFIG_DIR`, one per line.
struct Fixture {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    exe: PathBuf,
}

fn fixture(exe_name: &str, script: &str) -> Fixture {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("work dir 'q");
    let bin = tmp.path().join("bin dir 'q");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    let exe = bin.join(exe_name);
    std::fs::write(&exe, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    Fixture {
        _tmp: tmp,
        dir,
        exe,
    }
}

/// The lines a hint prints in `shell`: the context block's lines, then a rewritten command.
fn printed(shell: Shell, fx: &Fixture, config_dir: &str) -> String {
    let context = Context {
        dir: Some(fx.dir.clone()),
        exe: Some(fx.exe.clone()),
        config_dir: Some(config_dir.into()),
    };
    let block = hint::block(&context, shell);
    let command = hint::rewrite_for("sbxm task resume --issue 4", Some(&fx.exe), shell).unwrap();
    let mut script: String = block
        .lines()
        .skip(1)
        .map(|l| format!("{}\n", l.trim_start()))
        .collect();
    script.push_str(&command);
    script.push('\n');
    script
}

fn expected_output(fx: &Fixture, config_dir: &str) -> String {
    format!(
        "{}\ntask\nresume\n--issue\n4\n{config_dir}\n",
        fx.dir.display()
    )
}

#[cfg(unix)]
#[test]
fn the_sh_lines_run_as_written() {
    let fx = fixture(
        "my sbxm 'q",
        "#!/bin/sh\npwd\nfor a in \"$@\"; do echo \"$a\"; done\necho \"$SBXM_CONFIG_DIR\"\n",
    );
    let config_dir = "/cfg dir 'q";
    let script = printed(Shell::Sh, &fx, config_dir);
    let output = Command::new("sh").arg("-c").arg(&script).output().unwrap();
    assert!(output.status.success(), "{script}\n{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        expected_output(&fx, config_dir),
        "{script}"
    );
}

/// `pwsh` when it is installed. The PowerShell lines run through it where it runs scripts with a
/// `#!` line (not on Windows, where the string tests above stand for them).
fn pwsh() -> Option<&'static str> {
    Command::new("pwsh")
        .args(["-NoProfile", "-Command", "exit 0"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| "pwsh")
}

#[cfg(unix)]
#[test]
fn the_powershell_lines_run_as_written() {
    let Some(pwsh) = pwsh() else {
        eprintln!("no pwsh on this machine; the PowerShell lines are not run");
        return;
    };
    let fx = fixture(
        "my sbxm 'q",
        "#!/bin/sh\npwd\nfor a in \"$@\"; do echo \"$a\"; done\necho \"$SBXM_CONFIG_DIR\"\n",
    );
    let config_dir = "/cfg dir 'q";
    let script = printed(Shell::PowerShell, &fx, config_dir);
    let output = Command::new(pwsh)
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .unwrap();
    assert!(output.status.success(), "{script}\n{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        expected_output(&fx, config_dir),
        "{script}"
    );
}
