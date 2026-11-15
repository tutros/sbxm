//! Review M-2 of issue 667: the log a loop names to its child (`SBXM_LOOP_CHILD_LOG`) is that
//! child's alone. The child takes it out of its environment as it starts, so a command it runs
//! (a host gate's `cargo test`, `sbx`, a script) and any `sbxm` that command runs keep their own
//! stdout and stderr.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

use sbxm::screen::LOG_ENV;

use crate::common;

/// A `sbx` on `PATH` that, on every call, runs the nested `sbxm config profiles-dir`, keeping
/// what it printed on stdout and the log setting it saw, then answers as `sbx` would.
fn fake_sbx(dir: &Path) -> std::path::PathBuf {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let script = format!(
        "#!/bin/sh\n\
         \"{sbxm}\" config profiles-dir > \"{captured}\" 2>/dev/null\n\
         printf '%s' \"${LOG_ENV}\" > \"{seen}\"\n\
         case \"$*\" in\n\
         *version*) echo '{{\"client\":{{\"version\":\"v0.46.0\"}}}}';;\n\
         *) echo '{{\"sandboxes\":[]}}';;\n\
         esac\n",
        sbxm = env!("CARGO_BIN_EXE_sbxm"),
        captured = dir.join("captured").display(),
        seen = dir.join("seen").display(),
    );
    let sbx = bin.join("sbx");
    std::fs::write(&sbx, script).unwrap();
    std::fs::set_permissions(&sbx, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

#[test]
fn task_loop_child_env_a_command_the_child_runs_keeps_its_own_output() {
    let env = common::Env::new();
    let dir = env.tmp.path();
    let bin = fake_sbx(dir);
    let log = dir.join("child.log");

    // `sbxm doctor` as a loop child: it runs `sbx`, which runs a nested `sbxm`.
    let status = Command::new(env!("CARGO_BIN_EXE_sbxm"))
        .arg("doctor")
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .env(LOG_ENV, &log)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();

    let _ = status;
    let captured = std::fs::read_to_string(dir.join("captured")).unwrap();
    assert_eq!(
        captured.trim_end(),
        env.profiles_dir().display().to_string(),
        "the nested sbxm printed its path on its own stdout"
    );
    assert_eq!(std::fs::read_to_string(dir.join("seen")).unwrap(), "");
    // The child's own screen still went to its log.
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("sbx"), "{text}");
}
