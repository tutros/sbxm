//! The harnesses sbxm can create sandboxes for (decision 68).

use std::borrow::Cow;
use std::path::PathBuf;

use crate::config::Profile;

/// `sbx`'s built-in agent of the same name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum Harness {
    #[default]
    Claude,
    Codex,
    Gemini,
}

impl Harness {
    /// The `sbx` agent name, also used in sandbox names and state keys.
    pub fn as_str(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
            Harness::Gemini => "gemini",
        }
    }

    /// The always-loaded user-level instructions file, relative to home
    /// (decisions 37, 49).
    pub fn instructions_file(self) -> PathBuf {
        match self {
            Harness::Claude => PathBuf::from(".claude").join("CLAUDE.md"),
            Harness::Codex => PathBuf::from(".codex").join("AGENTS.md"),
            Harness::Gemini => PathBuf::from(".gemini").join("GEMINI.md"),
        }
    }

    /// Configured settings this harness can't apply, as `(key, what's lost)`,
    /// so they're warned about instead of dropped silently (decision 11).
    pub fn unsupported(self, profile: &Profile) -> Vec<(&'static str, &'static str)> {
        if self == Harness::Claude {
            return Vec::new();
        }
        let mut unsupported = Vec::new();
        if !profile.claude_home_files.is_empty() {
            unsupported.push(("harness.claude.home_files", "none of its files reach"));
        }
        if profile.claude_managed_settings.is_some() {
            unsupported.push((
                "harness.claude.managed_settings",
                "its settings (hooks included) don't reach",
            ));
        }
        unsupported
    }

    /// `profile` without the settings in [`Harness::unsupported`]: what
    /// reaches this harness's sandbox, and so what its kit and hash cover
    /// (decision 69).
    pub fn applied(self, profile: &Profile) -> Cow<'_, Profile> {
        if self == Harness::Claude {
            return Cow::Borrowed(profile);
        }
        let mut applied = profile.clone();
        applied.claude_home_files.clear();
        applied.claude_managed_settings = None;
        Cow::Owned(applied)
    }
}
