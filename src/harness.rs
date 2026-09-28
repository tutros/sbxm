//! The harnesses sbxm can create sandboxes for (decision 68).

use std::borrow::Cow;
use std::path::PathBuf;

use crate::backend::SkillsStore;
use crate::config::Profile;

/// `sbx`'s built-in agent of the same name, or Pi's kit (decision 73).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum Harness {
    #[default]
    Claude,
    Codex,
    Gemini,
    Pi,
}

/// The Pi kit on Docker Hub, pinned to an immutable tag (decision 73). `sbx`
/// allows `docker.io/` by default, so no `kit.allowedSources` change is needed.
const PI_KIT: &str = "docker.io/sbx/pi-kit:20260924-d058fedc156325f87612d9bcd9bd313ab74ba100";

impl Harness {
    /// The `sbx` agent name, also used in sandbox names and state keys.
    pub fn as_str(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
            Harness::Gemini => "gemini",
            Harness::Pi => "pi",
        }
    }

    /// The agent argument of `sbx create`: the kit ref for Pi, which isn't
    /// built into `sbx`.
    pub fn agent_arg(self) -> &'static str {
        match self {
            Harness::Pi => PI_KIT,
            other => other.as_str(),
        }
    }

    /// The always-loaded user-level instructions file, relative to home
    /// (decisions 37, 49).
    pub fn instructions_file(self) -> PathBuf {
        match self {
            Harness::Claude => PathBuf::from(".claude").join("CLAUDE.md"),
            Harness::Codex => PathBuf::from(".codex").join("AGENTS.md"),
            Harness::Gemini => PathBuf::from(".gemini").join("GEMINI.md"),
            Harness::Pi => PathBuf::from(".pi").join("agent").join("AGENTS.md"),
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
        // sbx writes agentInstructions above the workspace, and Pi reads
        // parent folders (decision 75).
        if self == Harness::Pi && profile.reference_instructions.is_some() {
            warnings.push(format!(
                "instructions.reference is set, and {name} sandboxes always load it (sbx writes \
                 it as AGENTS.md above the workspace, and Pi reads parent folders): it takes up \
                 context in every prompt in {sandbox}, not only on demand"
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
