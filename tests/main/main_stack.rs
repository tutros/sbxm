//! The CLI runs within a 1 MiB main-thread stack, the size Windows gives every process (PR 515's
//! failed check: clap's derive builds every `sbxm task` argument in one stack frame, which a debug
//! build no longer fit there). A Unix shell's `ulimit -s` sets the same limit here.

#[cfg(unix)]
#[test]
fn every_command_parses_within_a_one_mib_main_thread_stack() {
    let tmp = tempfile::TempDir::new().unwrap();
    for args in ["--help", "list", "task --help"] {
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("ulimit -s 1024 && exec \"$0\" {args}"))
            .arg(env!("CARGO_BIN_EXE_sbxm"))
            .env("SBXM_CONFIG_DIR", tmp.path())
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.contains("overflowed its stack"), "{args}: {stderr}");
        // `list` without a config fails on purpose, naming `sbxm config init`; the help succeeds.
        assert!(
            output.status.success() || stderr.contains(" config init`"),
            "{args}: {:?}\n{stderr}",
            output.status
        );
    }
}
