//! Issue #63 (M2a slice 12, decision 166): the pure parts of the cosine
//! evaluator - vector similarity and pairing contestants' answers within one
//! repeat index - against a fake `Embedder` (no model, no network).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;

use sbxm::eval::cosine::{self, CosineEntry, Embedder};
use sbxm::headless::{HeadlessResult, RunStatus, Usage};
use sbxm::run::orchestrate::PairOutcome;
use sbxm::run::results;
use serde_json::json;

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
fn a_missing_or_empty_answer_is_skipped_and_only_the_present_one_is_embedded() {
    let embedder = FakeEmbedder::new(&[("a", vec![1.0, 0.0])]);
    let missing = err_pair(0, 0);
    let empty = ok_pair(1, 0, "   ");
    let present = ok_pair(2, 0, "a");

    let entries = cosine::cosine_repeat(&embedder, &[&missing, &empty, &present]);

    assert_eq!(entries[&0], CosineEntry::Skipped);
    assert_eq!(entries[&1], CosineEntry::Skipped);
    // One real answer remains: it has no peers, but it still goes through the
    // embedder, so a model that cannot embed is recorded (review M-1, PR 69).
    assert_eq!(entries[&2], CosineEntry::Peers(Default::default()));
    assert_eq!(embedder.calls(), vec![vec!["a".to_owned()]]);
}

#[test]
fn a_single_answer_gets_no_peers() {
    let embedder = FakeEmbedder::new(&[("alone", vec![1.0, 0.0])]);
    let only = ok_pair(0, 0, "alone");

    let entries = cosine::cosine_repeat(&embedder, &[&only]);

    assert_eq!(entries[&0], CosineEntry::Peers(Default::default()));
    assert_eq!(embedder.calls(), vec![vec!["alone".to_owned()]]);
}

#[test]
fn a_single_answer_with_a_failing_embedder_records_the_error() {
    let embedder = FakeEmbedder::failing("boom");
    let only = ok_pair(0, 0, "alone");
    let nothing = err_pair(1, 0);

    let entries = cosine::cosine_repeat(&embedder, &[&only, &nothing]);

    assert_eq!(entries[&0], CosineEntry::Error("boom".into()));
    assert_eq!(entries[&1], CosineEntry::Skipped);
}

#[test]
fn no_answers_at_all_never_call_the_embedder() {
    let embedder = FakeEmbedder::failing("boom");
    let nothing = err_pair(0, 0);

    let entries = cosine::cosine_repeat(&embedder, &[&nothing]);

    assert_eq!(entries[&0], CosineEntry::Skipped);
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

// ---- display: reads saved files, never a model --------------------------------

fn peers(pairs: &[(usize, f32)]) -> BTreeMap<usize, f32> {
    pairs.iter().copied().collect()
}

#[test]
fn render_shows_each_pairs_similarity_by_contestant_index_with_two_decimals() {
    let meta = tempfile::TempDir::new().unwrap();
    let mut entries = BTreeMap::new();
    entries.insert(0, CosineEntry::Peers(peers(&[(1, 0.9305)])));
    entries.insert(1, CosineEntry::Peers(peers(&[(0, 0.9305)])));
    results::write_cosine(meta.path(), 0, &entries).unwrap();

    let text = cosine::render(meta.path(), 2, 1);

    assert_eq!(text, "Similarity repeat 1/1: 0-1 0.93\n");
}

#[test]
fn render_skips_a_repeat_with_no_cosine_data() {
    let meta = tempfile::TempDir::new().unwrap();

    let text = cosine::render(meta.path(), 2, 1);

    assert_eq!(text, "");
}

#[test]
fn render_shows_an_unavailable_line_when_every_entry_is_an_error() {
    let meta = tempfile::TempDir::new().unwrap();
    let mut entries = BTreeMap::new();
    entries.insert(0, CosineEntry::Error("boom".into()));
    entries.insert(1, CosineEntry::Error("boom".into()));
    results::write_cosine(meta.path(), 0, &entries).unwrap();

    let text = cosine::render(meta.path(), 2, 1);

    assert_eq!(text, "Similarity repeat 1/1: no comparable answers\n");
}

#[test]
fn render_shows_an_unavailable_line_when_every_entry_is_skipped() {
    let meta = tempfile::TempDir::new().unwrap();
    let mut entries = BTreeMap::new();
    entries.insert(0, CosineEntry::Skipped);
    entries.insert(1, CosineEntry::Skipped);
    results::write_cosine(meta.path(), 0, &entries).unwrap();

    let text = cosine::render(meta.path(), 2, 1);

    assert_eq!(text, "Similarity repeat 1/1: no comparable answers\n");
}

#[test]
fn render_shows_an_unavailable_line_for_a_lone_participant_with_no_peers() {
    let meta = tempfile::TempDir::new().unwrap();
    let mut entries = BTreeMap::new();
    entries.insert(0, CosineEntry::Peers(Default::default()));
    results::write_cosine(meta.path(), 0, &entries).unwrap();

    let text = cosine::render(meta.path(), 2, 1);

    assert_eq!(text, "Similarity repeat 1/1: no comparable answers\n");
}

#[test]
fn render_mixes_a_numeric_repeat_with_an_unavailable_one_and_skips_an_unconfigured_one() {
    let meta = tempfile::TempDir::new().unwrap();
    let mut numeric = BTreeMap::new();
    numeric.insert(0, CosineEntry::Peers(peers(&[(1, 0.5)])));
    numeric.insert(1, CosineEntry::Peers(peers(&[(0, 0.5)])));
    results::write_cosine(meta.path(), 0, &numeric).unwrap();
    let mut errored = BTreeMap::new();
    errored.insert(0, CosineEntry::Error("boom".into()));
    errored.insert(1, CosineEntry::Error("boom".into()));
    results::write_cosine(meta.path(), 1, &errored).unwrap();
    // Repeat 2 has no cosine entry at all (e.g. cosine wasn't configured for
    // this saved run): it must stay absent, not "no comparable answers".

    let text = cosine::render(meta.path(), 2, 3);

    assert_eq!(
        text,
        "Similarity repeat 1/3: 0-1 0.50\nSimilarity repeat 2/3: no comparable answers\n"
    );
}

#[test]
fn render_uses_blind_labels_when_the_judge_scored_that_repeat() {
    let meta = tempfile::TempDir::new().unwrap();
    let mut entries = BTreeMap::new();
    entries.insert(0, CosineEntry::Peers(peers(&[(1, 0.5)])));
    entries.insert(1, CosineEntry::Peers(peers(&[(0, 0.5)])));
    results::write_cosine(meta.path(), 0, &entries).unwrap();
    // Contestant 0 is judged under label B, contestant 1 under A.
    let judge_dir = meta.path().join("judge").join("0");
    std::fs::create_dir_all(&judge_dir).unwrap();
    std::fs::write(
        judge_dir.join("judge.json"),
        json!({"labels": {"B": 0, "A": 1}}).to_string(),
    )
    .unwrap();

    let text = cosine::render(meta.path(), 2, 1);

    assert_eq!(text, "Similarity repeat 1/1: A-B 0.50\n");
}

#[test]
fn render_never_opens_a_model_file() {
    // The signature alone proves it: no Embedder, no model_dir, just the
    // saved run folder and the shape of the run.
    let _: fn(&std::path::Path, usize, u32) -> String = cosine::render;
}

// ---- the real model (decision 166): needs model files on disk, no network ----

/// `cargo test --test eval_cosine -- --ignored`, with `SBXM_TEST_MODEL_DIR`
/// pointing at a folder holding the five files named in
/// `cosine::MODEL_FILES` (fetched by hand from `cosine::MODEL_URL_PREFIX`).
#[test]
#[ignore]
fn the_real_model_embeds_sentences_deterministically() {
    let dir = std::env::var("SBXM_TEST_MODEL_DIR")
        .expect("set SBXM_TEST_MODEL_DIR to a folder holding the cosine model files");
    let embedder = cosine::FastEmbedder::from_dir(std::path::Path::new(&dir)).unwrap();

    let texts = vec![
        "the cat sat on the mat".to_owned(),
        "a cat is sitting on a mat".to_owned(),
        "quarterly revenue grew twelve percent".to_owned(),
    ];
    let vectors = embedder.embed(&texts).unwrap();

    assert_eq!(vectors[0].len(), 384);
    let similar = cosine::similarity(&vectors[0], &vectors[1]).unwrap();
    assert!(similar > 0.9, "{similar}");
    let unrelated = cosine::similarity(&vectors[0], &vectors[2]).unwrap();
    assert!(unrelated < 0.1, "{unrelated}");

    let again = embedder.embed(&texts[..1]).unwrap();
    assert_eq!(again[0], vectors[0]);
}
