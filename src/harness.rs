//! The harnesses sbxm can create sandboxes for (decision 68).

use std::borrow::Cow;
use std::path::PathBuf;

use crate::backend::SkillsStore;
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

    /// A warning for each configured setting this harness can't apply in
    /// `sandbox`, so nothing is dropped silently (decision 11).
    pub fn unsupported(self, profile: &Profile, sandbox: &str) -> Vec<String> {
        let name = self.as_str();
        let mut warnings = Vec::new();
        if self != Harness::Claude {
            if !profile.claude_home_files.is_empty() {
                warnings.push(format!(
                    "harness.claude.home_files is set, but {name} sandboxes don't support it: \
                     none of its files reach {sandbox}"
                ));
            }
            if profile.claude_managed_settings.is_some() {
                warnings.push(format!(
                    "harness.claude.managed_settings is set, but {name} sandboxes don't support \
                     it: its settings (hooks included) don't reach {sandbox}"
                ));
            }
        }
        // The sbx skills store doesn't serve Gemini CLI (decisions 46, 72).
        if self == Harness::Gemini && profile.skills_store != SkillsStore::Off {
            warnings.push(format!(
                "skills.store is \"{}\", but {name} sandboxes don't support it: the sbx skills \
                 store's skills don't reach {sandbox}; set skills.store = \"off\" to silence this",
                profile.skills_store.as_arg()
            ));
        }
        warnings
    }

    /// `profile` without the Claude-only settings: what reaches this
    /// harness's sandbox, and so what its kit and hash cover (decision 69).
    /// `skills.store` stays, since it's still passed to `sbx create`.
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
