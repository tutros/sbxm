//! Cosine similarity between contestants' text answers (M2a slice 12, P4,
//! decision 166, issue #63): a signal of agreement, not quality, so it never
//! touches `eval::score`'s ranking. Fully local: no sandbox, no network once
//! the model files are on disk.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::run::orchestrate::PairOutcome;

/// Reported in every `evals.json["cosine"]["model"]`.
pub const MODEL_NAME: &str = "all-MiniLM-L6-v2";

/// Checked at preflight, in this order, so the first missing one is named.
pub const MODEL_FILES: [&str; 5] = [
    "model.onnx",
    "tokenizer.json",
    "config.json",
    "special_tokens_map.json",
    "tokenizer_config.json",
];

/// Where the model files come from (decision 166): `fastembed`'s own
/// downloader fails under TLS interception, so they're fetched by hand.
pub const MODEL_URL_PREFIX: &str =
    "https://huggingface.co/Qdrant/all-MiniLM-L6-v2-onnx/resolve/main/";

/// Turns text into vectors. `FastEmbedder` is the real one; tests use a fake.
pub trait Embedder {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
}

/// An `Embedder` that always fails with the same message, standing in for a
/// model that couldn't be loaded: every participant still gets a recorded
/// error instead of silently skipping cosine for the whole run.
pub struct FailingEmbedder(pub String);

impl Embedder for FailingEmbedder {
    fn embed(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Err(anyhow::anyhow!(self.0.clone()))
    }
}

/// `fastembed`'s `all-MiniLM-L6-v2`, loaded from local files (decision 166):
/// `fastembed`'s own downloader fails on a machine whose HTTPS is
/// intercepted. `embed` takes `&mut self` upstream, so the session sits
/// behind a `RefCell`; cosine only ever runs on one thread, after every pair
/// of a repeat has finished.
pub struct FastEmbedder {
    inner: std::cell::RefCell<fastembed::TextEmbedding>,
}

impl FastEmbedder {
    pub fn from_dir(dir: &Path) -> Result<Self> {
        let read = |name: &str| -> Result<Vec<u8>> {
            let path = dir.join(name);
            std::fs::read(&path).with_context(|| format!("cannot read {}", path.display()))
        };
        let tokenizer_files = fastembed::TokenizerFiles {
            tokenizer_file: read("tokenizer.json")?,
            config_file: read("config.json")?,
            special_tokens_map_file: read("special_tokens_map.json")?,
            tokenizer_config_file: read("tokenizer_config.json")?,
        };
        let model = fastembed::UserDefinedEmbeddingModel::new(read("model.onnx")?, tokenizer_files)
            .with_pooling(fastembed::Pooling::Mean);
        let embedding = fastembed::TextEmbedding::try_new_from_user_defined(
            model,
            fastembed::InitOptionsUserDefined::default(),
        )
        .with_context(|| format!("cannot load the cosine model from {}", dir.display()))?;
        Ok(FastEmbedder {
            inner: std::cell::RefCell::new(embedding),
        })
    }
}

impl Embedder for FastEmbedder {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.inner
            .borrow_mut()
            .embed(texts, None)
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }
}

/// The cosine similarity of `a` and `b`, or `None` if either is a zero
/// vector (undefined: there is no direction to compare).
pub fn similarity(a: &[f32], b: &[f32]) -> Option<f32> {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return None;
    }
    Some(dot / (norm_a * norm_b))
}

/// What `evals.json["cosine"]` holds for one contestant of one repeat.
#[derive(Debug, Clone, PartialEq)]
pub enum CosineEntry {
    /// Its similarity to every other contestant that also answered, by
    /// contestant index. Empty when it's the only one that did.
    Peers(BTreeMap<usize, f32>),
    /// No text answer (missing or empty `answer.md`): never an error.
    Skipped,
    /// The model couldn't be loaded, or embedding failed: never fails the run.
    Error(String),
}

impl CosineEntry {
    pub fn to_json(&self) -> Value {
        match self {
            CosineEntry::Peers(peers) => {
                let peers: serde_json::Map<String, Value> = peers
                    .iter()
                    .map(|(c, s)| (c.to_string(), json!(s)))
                    .collect();
                json!({"model": MODEL_NAME, "peers": peers})
            }
            CosineEntry::Skipped => json!({"skipped": "no text answer"}),
            CosineEntry::Error(message) => json!({"error": message}),
        }
    }
}

/// Compares every pair of `pairs` that has a non-empty text answer, all from
/// the same repeat index (the caller filters `pairs` to one; answers of
/// different indices must never meet). Never fails: a missing or empty
/// answer is `Skipped`, one remaining answer gets no peers, and an embedder
/// error is recorded for every would-be participant instead of propagating.
pub fn cosine_repeat(
    embedder: &dyn Embedder,
    pairs: &[&PairOutcome],
) -> BTreeMap<usize, CosineEntry> {
    let mut result = BTreeMap::new();
    let mut participants: Vec<(usize, String)> = Vec::new();
    for pair in pairs {
        let answer = pair.result.as_ref().ok().map(|r| r.answer.trim());
        match answer {
            Some(text) if !text.is_empty() => participants.push((pair.contestant, text.to_owned())),
            _ => {
                result.insert(pair.contestant, CosineEntry::Skipped);
            }
        }
    }
    if participants.len() < 2 {
        for (contestant, _) in &participants {
            result.insert(*contestant, CosineEntry::Peers(BTreeMap::new()));
        }
        return result;
    }

    let texts: Vec<String> = participants.iter().map(|(_, text)| text.clone()).collect();
    match embedder.embed(&texts) {
        Ok(vectors) => {
            for (i, (contestant, _)) in participants.iter().enumerate() {
                let mut peers = BTreeMap::new();
                for (j, (other, _)) in participants.iter().enumerate() {
                    if i == j {
                        continue;
                    }
                    if let Some(sim) = similarity(&vectors[i], &vectors[j]) {
                        peers.insert(*other, sim);
                    }
                }
                result.insert(*contestant, CosineEntry::Peers(peers));
            }
        }
        Err(e) => {
            let message = format!("{e:#}");
            for (contestant, _) in &participants {
                result.insert(*contestant, CosineEntry::Error(message.clone()));
            }
        }
    }
    result
}
