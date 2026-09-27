//! The harnesses sbxm can create sandboxes for (decision 68).

use std::path::PathBuf;

/// `sbx`'s built-in agent of the same name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum Harness {
    #[default]
    Claude,
    Codex,
}

impl Harness {
    /// The `sbx` agent name, also used in sandbox names and state keys.
    pub fn as_str(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
        }
    }

    /// The always-loaded user-level instructions file, relative to home
    /// (decisions 37, 49).
    pub fn instructions_file(self) -> PathBuf {
        match self {
            Harness::Claude => PathBuf::from(".claude").join("CLAUDE.md"),
            Harness::Codex => PathBuf::from(".codex").join("AGENTS.md"),
        }
    }
}
