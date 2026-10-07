//! Prompt templates (spec §10, decision 156): `{{name}}` placeholders over a fixed set of
//! names. A template is embedded (`prompts/*.md`) and a repo may override it with a file
//! inside the repo (`[prompts]` in `sbxm-task.toml`, already checked to exist and stay inside).

use std::path::Path;

use anyhow::{Context, Result, bail};

use super::config::Prompts;

/// The names a template may use; anything else is an error, so a typo is caught before a run.
const KNOWN: [&str; 12] = [
    "issue",
    "number",
    "branch",
    "base",
    "scope_base",
    "repo",
    "pr_context",
    "gates_sandbox",
    "gates_host",
    "review_path",
    "previous_review_path",
    "gate_output_path",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Worker,
    Reviewer,
    /// The reviewer of a pull request (no issue of its own, no fix round).
    ReviewerPr,
    /// The fix round after a review found must-fix findings.
    Fix,
    /// The fix round after a gate failure, which has no review to point at (issue 117).
    FixGate,
    /// The worker of a spec task (issue 142, decision 174(d)): the source is a file
    /// (`.sbxm-task/source.md`) instead of a GitHub issue, so the wording differs from
    /// [`Role::Worker`] even though the flow is the same.
    WorkerSpec,
    /// The reviewer of a spec task: no pull request to comment on, so `review.md` is the only
    /// output.
    ReviewerSpec,
    /// The fix round of a spec task after a review found must-fix findings.
    FixSpec,
    /// The fix round of a spec task after a gate failure.
    FixGateSpec,
}

/// A template and the name errors are reported under (the embedded name, or the override's path).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    pub name: String,
    pub text: String,
}

const WORKER: &str = include_str!("../../prompts/worker.md");
const REVIEWER: &str = include_str!("../../prompts/reviewer.md");
const REVIEWER_PR: &str = include_str!("../../prompts/reviewer-pr.md");
const FIX: &str = include_str!("../../prompts/fix.md");
const FIX_GATE: &str = include_str!("../../prompts/fix-gate.md");
const WORKER_SPEC: &str = include_str!("../../prompts/worker-spec.md");
const REVIEWER_SPEC: &str = include_str!("../../prompts/reviewer-spec.md");
const FIX_SPEC: &str = include_str!("../../prompts/fix-spec.md");
const FIX_GATE_SPEC: &str = include_str!("../../prompts/fix-gate-spec.md");

/// The template for `role`: the repo's override when configured, else the embedded one.
pub fn template(role: Role, prompts: &Prompts) -> Result<Template> {
    let (embedded_name, embedded, custom) = match role {
        Role::Worker => ("worker.md", WORKER, prompts.worker.as_deref()),
        Role::Reviewer => ("reviewer.md", REVIEWER, prompts.reviewer.as_deref()),
        // One `[prompts] reviewer` override covers issue and pull request reviews alike.
        Role::ReviewerPr => ("reviewer-pr.md", REVIEWER_PR, prompts.reviewer.as_deref()),
        Role::Fix => ("fix.md", FIX, prompts.fix.as_deref()),
        // One `[prompts] fix` override covers both kinds of fix round.
        Role::FixGate => ("fix-gate.md", FIX_GATE, prompts.fix.as_deref()),
        // A spec task reuses the same `[prompts]` override keys as an issue task's (issue 142):
        // one override point per role, whichever source a task started from.
        Role::WorkerSpec => ("worker-spec.md", WORKER_SPEC, prompts.worker.as_deref()),
        Role::ReviewerSpec => (
            "reviewer-spec.md",
            REVIEWER_SPEC,
            prompts.reviewer.as_deref(),
        ),
        Role::FixSpec => ("fix-spec.md", FIX_SPEC, prompts.fix.as_deref()),
        Role::FixGateSpec => ("fix-gate-spec.md", FIX_GATE_SPEC, prompts.fix.as_deref()),
    };
    match custom {
        Some(path) => read_override(path),
        None => Ok(Template {
            name: embedded_name.to_owned(),
            text: embedded.to_owned(),
        }),
    }
}

fn read_override(path: &Path) -> Result<Template> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read the prompt template {}", path.display()))?;
    Ok(Template {
        name: path.display().to_string(),
        text,
    })
}

/// Replaces every `{{name}}` in `template` with its value. A value is inserted as is and not
/// expanded again, so text from an issue can't inject a placeholder. `name` labels errors.
pub fn render(name: &str, template: &str, values: &[(&str, &str)]) -> Result<String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            bail!("prompt template {name} has an unclosed `{{{{`; close it with `}}}}`");
        };
        let placeholder = after[..end].trim();
        if !KNOWN.contains(&placeholder) {
            bail!(
                "prompt template {name} uses the unknown placeholder {{{{{placeholder}}}}}; \
                 known names: {}",
                KNOWN.join(", ")
            );
        }
        let Some((_, value)) = values.iter().find(|(key, _)| *key == placeholder) else {
            bail!("prompt template {name} uses {{{{{placeholder}}}}}, which has no value here");
        };
        out.push_str(value);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}
