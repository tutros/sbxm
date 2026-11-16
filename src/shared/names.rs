//! Names both sides check or print: a usable git ref, and how an agent is named in output.

/// A branch or ref name that is safe to hand to git as an argument: no leading `-`, no
/// spaces or shell-ish characters, none of git's forbidden sequences.
pub fn valid_ref_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(['-', '/', '.'])
        && !name.ends_with(['/', '.'])
        && !name.ends_with(".lock")
        && !name.contains("..")
        && !name.contains("//")
        && !name.contains("@{")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
}

/// How a worker or reviewer is named in output: `antigravity (gemini-3.8-flash-medium)`, or
/// `claude (default model)` when the harness picks its own (issue 314).
pub fn agent_label(harness: &str, model: Option<&str>) -> String {
    format!("{harness} ({})", model.unwrap_or("default model"))
}
