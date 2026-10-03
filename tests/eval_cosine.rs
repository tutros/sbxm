//! Issue #63 (M2a slice 12, decision 166): the pure parts of the cosine
//! evaluator - vector similarity and pairing contestants' answers within one
//! repeat index - against a fake `Embedder` (no model, no network).

use std::cell::RefCell;
use std::path::PathBuf;

use sbxm::eval::cosine::{self, CosineEntry, Embedder};
use sbxm::headless::{HeadlessResult, RunStatus, Usage};
use sbxm::run::orchestrate::PairOutcome;

/// Returns a fixed vector per text (looked up by exact match) and records
/// every call, so a test can assert whether (and with what) it was invoked.
struct FakeEmbedder {
    vectors: Vec<(String, Vec<f32>)>,
    fail: Option<String>,
    calls: RefCell<Vec<Vec<String>>>,
}

impl FakeEmbedder {
    fn new(vectors: &[(&str, Vec<f32>)]) -> Self {
        FakeEmbedder {
            vectors: vectors
                .iter()
                .map(|(t, v)| (t.to_string(), v.clone()))
                .collect(),
            fail: None,
            calls: RefCell::new(Vec::new()),
        }
    }

    fn failing(message: &str) -> Self {
        FakeEmbedder {
            vectors: Vec::new(),
            fail: Some(message.to_owned()),
            calls: RefCell::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<Vec<String>> {
        self.calls.borrow().clone()
    }
}

impl Embedder for FakeEmbedder {
    fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        self.calls.borrow_mut().push(texts.to_vec());
        if let Some(message) = &self.fail {
            return Err(anyhow::anyhow!(message.clone()));
        }
        Ok(texts
            .iter()
            .map(|t| {
                self.vectors
                    .iter()
                    .find(|(text, _)| text == t)
                    .unwrap_or_else(|| panic!("no fake vector for {t:?}"))
                    .1
                    .clone()
            })
            .collect())
    }
}

// ---- similarity --------------------------------------------------------------

#[test]
fn identical_vectors_are_perfectly_similar() {
    assert_eq!(
        cosine::similarity(&[1.0, 0.0, 0.0], &[1.0, 0.0, 0.0]),
        Some(1.0)
    );
}

#[test]
fn orthogonal_vectors_have_zero_similarity() {
    assert_eq!(cosine::similarity(&[1.0, 0.0], &[0.0, 1.0]), Some(0.0));
}

#[test]
fn opposite_vectors_have_similarity_negative_one() {
    assert_eq!(cosine::similarity(&[1.0, 0.0], &[-1.0, 0.0]), Some(-1.0));
}

#[test]
fn a_zero_vector_gives_none() {
    assert_eq!(cosine::similarity(&[0.0, 0.0], &[1.0, 0.0]), None);
    assert_eq!(cosine::similarity(&[0.0, 0.0], &[0.0, 0.0]), None);
}

// ---- pairing one repeat's answers ---------------------------------------------

fn ok_pair(contestant: usize, repeat: u32, answer: &str) -> PairOutcome {
    PairOutcome {
        contestant,
        repeat,
        profile: "default".into(),
        sandbox: String::new(),
        workspace: PathBuf::new(),
        result: Ok(HeadlessResult {
            status: RunStatus::Completed,
            answer: answer.into(),
            transcript: String::new(),
            usage: Usage {
                input_tokens: 0,
                output_tokens: 0,
                cost_usd: None,
            },
        }),
        diff: None,
        checks: Vec::new(),
        remove_error: None,
        save_error: None,
    }
}

fn err_pair(contestant: usize, repeat: u32) -> PairOutcome {
    PairOutcome {
        contestant,
        repeat,
        profile: "default".into(),
        sandbox: String::new(),
        workspace: PathBuf::new(),
        result: Err("the pair never ran".into()),
        diff: None,
        checks: Vec::new(),
        remove_error: None,
        save_error: None,
    }
}

#[test]
fn two_answers_become_each_others_only_peer() {
    let embedder = FakeEmbedder::new(&[("a", vec![1.0, 0.0]), ("b", vec![0.0, 1.0])]);
    let p0 = ok_pair(0, 0, "a");
    let p1 = ok_pair(1, 0, "b");

    let entries = cosine::cosine_repeat(&embedder, &[&p0, &p1]);

    match &entries[&0] {
        CosineEntry::Peers(peers) => assert_eq!(peers[&1], 0.0),
        other => panic!("{other:?}"),
    }
    match &entries[&1] {
        CosineEntry::Peers(peers) => assert_eq!(peers[&0], 0.0),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_missing_or_empty_answer_is_skipped_without_calling_the_embedder() {
    let embedder = FakeEmbedder::new(&[("a", vec![1.0, 0.0])]);
    let missing = err_pair(0, 0);
    let empty = ok_pair(1, 0, "   ");
    let present = ok_pair(2, 0, "a");

    let entries = cosine::cosine_repeat(&embedder, &[&missing, &empty, &present]);

    assert_eq!(entries[&0], CosineEntry::Skipped);
    assert_eq!(entries[&1], CosineEntry::Skipped);
    // Only one real answer remains, so no embedding call was needed at all.
    assert_eq!(entries[&2], CosineEntry::Peers(Default::default()));
    assert!(embedder.calls().is_empty(), "{:?}", embedder.calls());
}

#[test]
fn a_single_answer_gets_no_peers() {
    let embedder = FakeEmbedder::new(&[]);
    let only = ok_pair(0, 0, "alone");

    let entries = cosine::cosine_repeat(&embedder, &[&only]);

    assert_eq!(entries[&0], CosineEntry::Peers(Default::default()));
    assert!(embedder.calls().is_empty());
}

#[test]
fn answers_of_one_repeat_are_never_sent_with_another_repeats() {
    let embedder = FakeEmbedder::new(&[
        ("r0-a", vec![1.0, 0.0]),
        ("r0-b", vec![0.0, 1.0]),
        ("r1-a", vec![1.0, 0.0]),
        ("r1-b", vec![0.0, 1.0]),
    ]);
    let pairs = [
        ok_pair(0, 0, "r0-a"),
        ok_pair(1, 0, "r0-b"),
        ok_pair(0, 1, "r1-a"),
        ok_pair(1, 1, "r1-b"),
    ];

    for repeat in 0..2 {
        let of_repeat: Vec<&PairOutcome> = pairs.iter().filter(|p| p.repeat == repeat).collect();
        cosine::cosine_repeat(&embedder, &of_repeat);
    }

    let calls = embedder.calls();
    assert_eq!(calls.len(), 2, "{calls:?}");
    for call in &calls {
        let is_repeat_0 = call.iter().all(|t| t.starts_with("r0-"));
        let is_repeat_1 = call.iter().all(|t| t.starts_with("r1-"));
        assert!(is_repeat_0 || is_repeat_1, "{call:?}");
    }
}

#[test]
fn an_embedder_error_is_recorded_for_every_participant() {
    let embedder = FakeEmbedder::failing("boom");
    let p0 = ok_pair(0, 0, "a");
    let p1 = ok_pair(1, 0, "b");
    let skipped = err_pair(2, 0);

    let entries = cosine::cosine_repeat(&embedder, &[&p0, &p1, &skipped]);

    assert_eq!(entries[&0], CosineEntry::Error("boom".into()));
    assert_eq!(entries[&1], CosineEntry::Error("boom".into()));
    assert_eq!(entries[&2], CosineEntry::Skipped);
}

// ---- JSON shape ----------------------------------------------------------------

#[test]
fn the_json_shapes_match_the_spec() {
    let mut peers = std::collections::BTreeMap::new();
    peers.insert(1usize, 0.933_f32);
    assert_eq!(
        CosineEntry::Peers(peers).to_json(),
        serde_json::json!({"model": "all-MiniLM-L6-v2", "peers": {"1": 0.933_f32}})
    );
    assert_eq!(
        CosineEntry::Peers(Default::default()).to_json(),
        serde_json::json!({"model": "all-MiniLM-L6-v2", "peers": {}})
    );
    assert_eq!(
        CosineEntry::Skipped.to_json(),
        serde_json::json!({"skipped": "no text answer"})
    );
    assert_eq!(
        CosineEntry::Error("oops".into()).to_json(),
        serde_json::json!({"error": "oops"})
    );
}
