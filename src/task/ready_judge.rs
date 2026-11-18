//! The model half of `sbxm task ready-check --judge` (issue 726, decision 331(2)): the judgment
//! rules (open points in the prose, boundary cases under decision 244, the acceptance list against
//! the answers, a proposed `**Files:**` line, the size's fit) go to one model call in a fresh
//! sandbox that reaches only the model provider (decision 269(c): issue text is untrusted). Its
//! answer has a fixed format, parsed here. It advises: it never sets a label or gates the loop.

use super::finish::next_fence;
use super::ready::files_well_formed;

/// The embedded prompt (`prompts/ready-check.md`): the readiness rules and the answer's format.
pub const PROMPT: &str = include_str!("../../prompts/ready-check.md");

/// The answer's sections, in their order.
pub const SECTIONS: [&str; 5] = [
    "files",
    "open_points",
    "boundary_cases",
    "acceptance_vs_answers",
    "size",
];

/// The workspace files the prompt names: the issue, the default branch's paths, and the open PRs'
/// changed files.
pub const ISSUE_FILE: &str = "issue.md";
pub const PATHS_FILE: &str = "paths.txt";
pub const OPEN_PRS_FILE: &str = "open-prs.md";

/// A parsed answer: each section's text, trimmed, its lines kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// The proposed paths, as a `**Files:**` line's text, or `none`.
    pub files: String,
    pub open_points: String,
    pub boundary_cases: String,
    pub acceptance_vs_answers: String,
    pub size: String,
}

impl Answer {
    /// The `**Files:**` line the answer proposes; `None` when it proposes none.
    pub fn files_line(&self) -> Option<String> {
        (!self.files.eq_ignore_ascii_case("none")).then(|| format!("**Files:** {}", self.files))
    }

    /// The sections as `name: text`, in their order.
    pub fn sections(&self) -> [(&'static str, &str); 5] {
        [
            (SECTIONS[0], &self.files),
            (SECTIONS[1], &self.open_points),
            (SECTIONS[2], &self.boundary_cases),
            (SECTIONS[3], &self.acceptance_vs_answers),
            (SECTIONS[4], &self.size),
        ]
    }
}

/// Reads the model's answer: every section once, none empty, `files:` one line of backticked
/// paths or `none`. Text before the first section and code fence lines are left out. `Err` says
/// why it doesn't parse.
pub fn parse(text: &str) -> Result<Answer, String> {
    let mut found: Vec<(&str, Vec<&str>)> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim_start().starts_with("```") {
            continue;
        }
        let header = SECTIONS.iter().find_map(|name| {
            line.strip_prefix(name)
                .and_then(|rest| rest.strip_prefix(':'))
                .map(|rest| (*name, rest))
        });
        match header {
            Some((name, rest)) => {
                if found.iter().any(|(n, _)| *n == name) {
                    return Err(format!("the section `{name}:` is there twice"));
                }
                found.push((name, vec![rest.trim()]));
            }
            None => {
                if let Some((_, lines)) = found.last_mut() {
                    lines.push(line);
                }
            }
        }
    }
    let missing: Vec<String> = SECTIONS
        .iter()
        .filter(|name| !found.iter().any(|(n, _)| n == *name))
        .map(|name| format!("`{name}:`"))
        .collect();
    if !missing.is_empty() {
        return Err(format!("no {} section", missing.join(", ")));
    }
    let text_of = |name: &str| -> Result<String, String> {
        let lines = &found.iter().find(|(n, _)| *n == name).unwrap().1;
        let text = lines.join("\n").trim().to_owned();
        if text.is_empty() {
            return Err(format!(
                "the section `{name}:` is empty (it should say `none`)"
            ));
        }
        Ok(text)
    };
    let files = text_of("files")?;
    if !files.eq_ignore_ascii_case("none") && (files.contains('\n') || !files_well_formed(&files)) {
        return Err(format!(
            "`files:` `{files}` is neither `none` nor one line of backticked paths separated by \
             commas"
        ));
    }
    Ok(Answer {
        files,
        open_points: text_of("open_points")?,
        boundary_cases: text_of("boundary_cases")?,
        acceptance_vs_answers: text_of("acceptance_vs_answers")?,
        size: text_of("size")?,
    })
}

/// `body` with `line`, a `**Files:**` line, in place of its first `**Files:**` line outside code
/// fences, or else added before its first `**Depends on:**` line, with a blank line after it, or
/// else at its end. Line endings are kept.
pub fn with_files_line(body: &str, line: &str) -> String {
    let lines: Vec<&str> = body.split_inclusive('\n').collect();
    let mut fence = None;
    let mut unfenced = Vec::new();
    for (at, text) in lines.iter().enumerate() {
        let inside = fence.is_some();
        fence = next_fence(fence, text.trim_end_matches(['\r', '\n']));
        if !(inside || fence.is_some()) {
            unfenced.push(at);
        }
    }
    let starts = |at: &usize, label: &str| lines[*at].trim_start().starts_with(label);
    let ending = |text: &str| {
        let content = text.trim_end_matches(['\r', '\n']);
        text[content.len()..].to_owned()
    };
    if let Some(at) = unfenced.iter().find(|at| starts(at, "**Files:**")) {
        let mut out: Vec<String> = lines.iter().map(|l| (*l).to_owned()).collect();
        out[*at] = format!("{line}{}", ending(lines[*at]));
        return out.concat();
    }
    if let Some(at) = unfenced.iter().find(|at| starts(at, "**Depends on:**")) {
        let mut eol = ending(lines[*at]);
        if eol.is_empty() {
            eol = "\n".to_owned();
        }
        let mut out: Vec<String> = lines.iter().map(|l| (*l).to_owned()).collect();
        out.insert(*at, format!("{line}{eol}{eol}"));
        return out.concat();
    }
    let body = body.trim_end_matches(['\r', '\n']);
    format!("{body}\n\n{line}\n")
}
