//! Issue 419: a headless run reports the model the harness says it used, from its own output
//! (Claude's `system`/`init` event, Antigravity's `init` event), or none when the output names
//! none (Codex's `exec --json` never does).

use sbxm::harness::Harness;

const CLAUDE: &str = include_str!("../../src/fixtures/claude-stream-json-pong.jsonl");
const CLAUDE_TRUNCATED: &str =
    include_str!("../../src/fixtures/claude-stream-json-truncated.jsonl");
const CODEX: &str = include_str!("../../src/fixtures/codex-ndjson-pong.jsonl");
const AGY: &str = include_str!("../../src/fixtures/antigravity-stream-json-tool.jsonl");
/// An init event without a model (an older `agy`).
const AGY_NO_MODEL: &str = include_str!("../../src/fixtures/antigravity-stream-json-pong.jsonl");

fn model(harness: Harness, raw: &str) -> Option<String> {
    harness.parse_headless_output(raw).unwrap().model
}

#[test]
fn headless_model_claude_is_read_from_its_init_event() {
    assert_eq!(
        model(Harness::Claude, CLAUDE).as_deref(),
        Some("claude-haiku-4-5-20251001")
    );
}

#[test]
fn headless_model_claude_is_kept_from_a_truncated_stream() {
    assert_eq!(
        model(Harness::Claude, CLAUDE_TRUNCATED).as_deref(),
        Some("claude-haiku-4-5-20251001")
    );
}

#[test]
fn headless_model_claude_falls_back_to_the_assistant_messages_model() {
    let raw = "{\"type\":\"assistant\",\"message\":{\"model\":\"claude-x\",\"content\":[]}}\n";
    assert_eq!(model(Harness::Claude, raw).as_deref(), Some("claude-x"));
}

#[test]
fn headless_model_claude_output_without_one_has_none() {
    assert_eq!(model(Harness::Claude, "{\"type\":\"result\"}\n"), None);
    assert_eq!(model(Harness::Claude, ""), None);
}

#[test]
fn headless_model_codex_names_none() {
    assert_eq!(model(Harness::Codex, CODEX), None);
}

#[test]
fn headless_model_antigravity_is_read_from_its_init_event() {
    assert_eq!(
        model(Harness::Antigravity, AGY).as_deref(),
        Some("gemini-3.8-flash-low")
    );
    assert_eq!(model(Harness::Antigravity, AGY_NO_MODEL), None);
}
