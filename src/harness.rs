//! The harnesses sbxm can create sandboxes for (decision 68).

use std::borrow::Cow;
use std::path::PathBuf;

use anyhow::{Result, bail};

use crate::backend::{SkillsStore, Stdin};
use crate::config::Profile;
use crate::headless::{self, HeadlessOpts, HeadlessResult};

/// `sbx`'s built-in agent of the same name, or Pi's kit (decision 73).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum Harness {
    #[default]
    Claude,
    Codex,
    Gemini,
    Pi,
    Antigravity,
}

/// The Pi kit on Docker Hub, pinned to an immutable tag (decision 73). `sbx`
/// allows `docker.io/` by default, so no `kit.allowedSources` change is needed.
const PI_KIT: &str = "docker.io/sbx/pi-kit:20260924-d058fedc156325f87612d9bcd9bd313ab74ba100";

/// The Antigravity kit on Docker Hub, pinned to an immutable tag (decision
/// 104; the same image as `latest` on 2026-09-28, looked up 2026-09-29).
const ANTIGRAVITY_KIT: &str =
    "docker.io/sbx/antigravity-kit:20260928-7da9425e640d172e36abe6de3499a1c75d416c0f";

impl Harness {
    /// The `sbx` agent name, also used in sandbox names and state keys.
    pub fn as_str(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
            Harness::Gemini => "gemini",
            Harness::Pi => "pi",
            Harness::Antigravity => "antigravity",
        }
    }

    /// The agent argument of `sbx create`: the kit ref for Pi, which isn't
    /// built into `sbx`.
    pub fn agent_arg(self) -> &'static str {
        self.agent_kit().unwrap_or_else(|| self.as_str())
    }

    /// The `sbx` service secret this harness's provider calls need (decision
    /// 98). Antigravity uses it as a Gemini API key (the kit sets
    /// `modelProvider: gemini`; decision 137).
    pub fn provider_secret(self) -> &'static str {
        match self {
            Harness::Claude | Harness::Pi => "anthropic",
            Harness::Codex => "openai",
            Harness::Gemini | Harness::Antigravity => "google",
        }
    }

    /// The pinned external kit this harness's sandbox is created from, if it
    /// isn't an `sbx` built-in. Part of the config hash, so a re-pin shows as
    /// drift (decisions 73, 104).
    pub fn agent_kit(self) -> Option<&'static str> {
        match self {
            Harness::Pi => Some(PI_KIT),
            Harness::Antigravity => Some(ANTIGRAVITY_KIT),
            _ => None,
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
            // Verified on real sbx (M2a P5): `agy -p` loads `~/.gemini/AGENTS.md`
            // (and GEMINI.md), but not the same names in `~/.gemini/antigravity-cli/`.
            Harness::Antigravity => PathBuf::from(".gemini").join("AGENTS.md"),
        }
    }

    /// The headless command `headless::run` executes in the sandbox (S5).
    pub fn headless_argv(self, prompt: &str, opts: &HeadlessOpts) -> Result<Vec<String>> {
        self.require_headless()?;
        let mut argv: Vec<String> = match self {
            Harness::Codex => ["codex", "exec", "-m", &opts.model, "--json"]
                .map(str::to_owned)
                .into(),
            Harness::Antigravity => ["agy", "-p", prompt, "--model", &opts.model]
                .into_iter()
                .chain(["--output-format", "stream-json"])
                .chain(["--dangerously-skip-permissions"])
                .map(str::to_owned)
                .collect(),
            _ => ["claude", "-p", prompt, "--model", &opts.model]
                .into_iter()
                .chain(["--output-format", "stream-json", "--verbose"])
                .map(str::to_owned)
                .collect(),
        };
        argv.extend(
            self.git_repo_workaround(opts.is_git_repo)
                .map(str::to_owned),
        );
        argv.extend(self.budget_flag(opts.budget_usd).unwrap_or_default());
        if self == Harness::Codex {
            argv.push(prompt.to_owned());
        }
        Ok(argv)
    }

    /// How stdin is wired for the headless command. `codex exec` blocks on a
    /// stdin that is attached but never closed, so it gets an empty pipe (S5).
    pub fn stdin(self) -> Stdin {
        match self {
            Harness::Codex => Stdin::Piped(String::new()),
            _ => Stdin::Closed,
        }
    }

    /// The flag that lets the command run outside a git repository, needed
    /// for an unseeded workspace (decision 113).
    pub fn git_repo_workaround(self, is_git_repo: bool) -> Option<&'static str> {
        match (self, is_git_repo) {
            (Harness::Codex, false) => Some("--skip-git-repo-check"),
            _ => None,
        }
    }

    /// The flag that caps a run's cost, if the harness has one (S5).
    pub fn budget_flag(self, budget_usd: Option<f64>) -> Option<Vec<String>> {
        match (self, budget_usd) {
            (Harness::Claude, Some(usd)) => {
                Some(vec!["--max-budget-usd".to_owned(), usd.to_string()])
            }
            _ => None,
        }
    }

    /// Parses the headless command's stdout, best-effort on truncated output.
    pub fn parse_headless_output(self, raw: &str) -> Result<HeadlessResult> {
        self.require_headless()?;
        Ok(match self {
            Harness::Codex => headless::parse_codex(raw),
            Harness::Antigravity => headless::parse_antigravity(raw),
            _ => headless::parse_claude(raw),
        })
    }

    fn require_headless(self) -> Result<()> {
        if !matches!(
            self,
            Harness::Claude | Harness::Codex | Harness::Antigravity
        ) {
            bail!(
                "{} has no headless adapter; use claude, codex or antigravity",
                self.as_str()
            );
        }
        Ok(())
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
        // The sbx skills store doesn't serve Gemini CLI or Pi (decisions 46, 72, 78),
        // and isn't known to serve Antigravity (not on decision 46's list; an empty
        // store can't be observed without changing it).
        if matches!(self, Harness::Gemini | Harness::Pi | Harness::Antigravity)
            && profile.skills_store != SkillsStore::Off
        {
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
