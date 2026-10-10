//! The risk assessment of a task's change (decision 176, issue 123): the reviewer assesses in
//! `review.md`, and sbxm raises the level to a minimum from path rules, but never lowers it.
//! Information only: the level changes nothing else sbxm does.

use serde::{Deserialize, Serialize};

/// `low`, `medium` or `high`, as the reviewer writes it; `unknown` is sbxm's own value for a
/// review whose `Risk:` line is missing or unparseable (never silently `low`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    #[default]
    Unknown,
    Low,
    Medium,
    High,
}

impl Level {
    pub fn word(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    /// GitHub Markdown cannot color text (decision 176(b)), so the level is shown with a marker.
    pub fn marker(self) -> &'static str {
        match self {
            Self::Unknown => "\u{26aa}", // ⚪
            Self::Low => "\u{1f7e2}",    // 🟢
            Self::Medium => "\u{1f7e1}", // 🟡
            Self::High => "\u{1f534}",   // 🔴
        }
    }

    /// `low`, `medium` or `high` only: a reviewer never writes `unknown` itself (decision 176(e)).
    pub fn from_word(word: &str) -> Option<Self> {
        match word {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }
}

/// A path-rule floor (decision 176(d)): any changed path containing `pattern` (matched
/// case-insensitively) raises the level to at least `level`.
#[derive(Debug, Clone)]
pub struct PathRule {
    pub level: Level,
    pub pattern: String,
    /// Shown in the reasons when this rule raises the level, naming the category (e.g. "a CI or
    /// workflow file").
    pub category: String,
}

/// Language-neutral defaults (decision 176(d)): CI and workflow files, dependency manifests and
/// lockfiles, and files named like secrets. All are a `Medium` floor; a repo's own especially
/// sensitive paths go in its `sbxm-task.toml` `[risk]` (often at `high`).
pub fn built_in_rules() -> Vec<PathRule> {
    let ci = [
        ".github/workflows/",
        ".github/actions/",
        ".gitlab-ci.yml",
        ".circleci/",
        "azure-pipelines.yml",
        "jenkinsfile",
    ];
    let dependencies = [
        "cargo.lock",
        "cargo.toml",
        "package.json",
        "package-lock.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "go.mod",
        "go.sum",
        "gemfile",
        "requirements.txt",
        "pipfile",
        "poetry.lock",
        "composer.json",
        "composer.lock",
        "pom.xml",
        "pyproject.toml",
        "setup.py",
        "setup.cfg",
        "uv.lock",
        "pdm.lock",
        "build.gradle",
        "settings.gradle",
        "gradle.lockfile",
        "package.swift",
        "package.resolved",
        "podfile",
        ".csproj",
        "packages.lock.json",
        "directory.packages.props",
        "pubspec.yaml",
        "pubspec.lock",
        "mix.exs",
        "mix.lock",
        "bun.lockb",
        "bun.lock",
        "flake.lock",
        "vcpkg.json",
        "conanfile",
    ];
    let secrets = [".env", "secret", "credential", "password", "apikey"];
    let mut rules = Vec::new();
    for (patterns, category) in [
        (&ci[..], "a CI or workflow file"),
        (&dependencies[..], "a dependency manifest or lockfile"),
        (&secrets[..], "a file named like a secret"),
    ] {
        for pattern in patterns {
            rules.push(PathRule {
                level: Level::Medium,
                pattern: pattern.to_string(),
                category: category.to_owned(),
            });
        }
    }
    rules
}

/// The highest level any of `rules` floors `paths` to, and one reason per matching category
/// (several files, or several rules, matching the same category still give one reason: the
/// highest level that category reached and a path that matched it, so the reason never
/// understates what the level ended up being). `Level::Unknown` (no rule matched anything)
/// carries no reasons.
pub fn floor(paths: &[String], rules: &[PathRule]) -> (Level, Vec<String>) {
    let mut level = Level::Unknown;
    // One (category, level, reason) entry per category, in first-seen order; a later match in
    // the same category only replaces the reason when its level is higher than what's recorded.
    let mut by_category: Vec<(String, Level, String)> = Vec::new();
    for path in paths {
        let lower = path.to_lowercase();
        for rule in rules {
            if !lower.contains(&rule.pattern) {
                continue;
            }
            level = level.max(rule.level);
            let reason = format!(
                "`{path}` is {} (at least {})",
                rule.category,
                rule.level.word()
            );
            match by_category.iter_mut().find(|(c, ..)| c == &rule.category) {
                Some(entry) if entry.1 < rule.level => {
                    *entry = (rule.category.clone(), rule.level, reason)
                }
                Some(_) => {}
                None => by_category.push((rule.category.clone(), rule.level, reason)),
            }
        }
    }
    let reasons = by_category
        .into_iter()
        .map(|(_, _, reason)| reason)
        .collect();
    (level, reasons)
}

/// The short section at the top of a PR description or review comment (decision 176(b)): the
/// marker, the level, then one bullet per reason.
pub fn render_section(level: Level, reasons: &[String]) -> String {
    let mut text = format!("## Risk: {} {}\n", level.marker(), level.word());
    if reasons.is_empty() {
        text.push_str("\n(no reasons given)\n");
    } else {
        text.push('\n');
        for reason in reasons {
            text.push_str(&format!("- {reason}\n"));
        }
    }
    text
}

/// [`render_section`]'s output is capped well below GitHub's limit (issue 123, review finding
/// M-4): without a cap of its own, a reviewer's overlong risk reason could by itself push a PR
/// body or review comment past what GitHub accepts, even though every other section there has
/// its own cap.
pub const RISK_CAP: usize = 3_000;

/// `text` (typically [`render_section`]'s output), cut to [`RISK_CAP`] characters with a note if
/// it had to be. Returns the (possibly cut) text and, when it was cut, one line describing it
/// for the caller to report.
pub fn cap_section(text: &str) -> (String, Option<String>) {
    let text = text.trim_end();
    let total = text.chars().count();
    if total <= RISK_CAP {
        return (text.to_owned(), None);
    }
    let head: String = text.chars().take(RISK_CAP).collect();
    let capped = format!(
        "{}\n\n(cut: the risk section has {total} characters; the first {RISK_CAP} are shown, \
         the rest is in the task folder)\n",
        head.trim_end()
    );
    let note =
        format!("the risk section is {total} characters; only the first {RISK_CAP} are shown");
    (capped, Some(note))
}
