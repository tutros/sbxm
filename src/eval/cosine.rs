//! Cosine similarity between contestants' text answers (M2a slice 12, P4,
//! decision 166, issue #63): a signal of agreement, not quality, so it never
//! touches `eval::score`'s ranking. Fully local: no sandbox, no network once
//! the model files are on disk.

use std::collections::BTreeMap;
use std::fmt::Write as _;
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

/// Lets a borrowed `Embedder` stand in wherever a `Box<dyn Embedder>` is
/// expected (`commands::run::run_with_embedder`'s test seam, issue #63 review
/// M-1): an already-loaded fake can be passed by reference instead of boxed.
impl Embedder for &dyn Embedder {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        (**self).embed(texts)
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

/// One similarity line per repeat that has cosine data, after the ranking
/// (issue #63): the pairs' values, two decimals, in blind-label order when
/// the judge scored that repeat (the mapping comes from its saved
/// `judge/<repeat>/judge.json`), else by contestant index. A repeat whose
/// saved files hold a `cosine` entry but no numeric pair (every participant
/// errored, was skipped, or only one answered) still gets a line, saying so;
/// a repeat with no `cosine` entry at all (cosine wasn't configured, or
/// nothing has been saved yet) is left out, same as before. Only reads saved
/// files; never loads a model.
pub fn render(meta: &Path, contestants: usize, repeats: u32) -> String {
    let mut out = String::new();
    for repeat in 0..repeats {
        let Some(pairs) = load_repeat(meta, contestants, repeat) else {
            continue;
        };
        if pairs.is_empty() {
            let _ = writeln!(
                out,
                "Similarity repeat {}/{repeats}: no comparable answers",
                repeat + 1
            );
            continue;
        }
        let labels = judge_labels(meta, repeat);
        let mut parts: Vec<(String, f32)> = pairs
            .into_iter()
            .map(|(a, b, sim)| {
                let name = match &labels {
                    Some(l) => {
                        let mut letters = [
                            l.get(&a).copied().unwrap_or('?'),
                            l.get(&b).copied().unwrap_or('?'),
                        ];
                        letters.sort_unstable();
                        format!("{}-{}", letters[0], letters[1])
                    }
                    None => format!("{a}-{b}"),
                };
                (name, sim)
            })
            .collect();
        parts.sort_by(|x, y| x.0.cmp(&y.0));
        let text: Vec<String> = parts.iter().map(|(n, s)| format!("{n} {s:.2}")).collect();
        let _ = writeln!(
            out,
            "Similarity repeat {}/{repeats}: {}",
            repeat + 1,
            text.join(", ")
        );
    }
    out
}

/// Each unique (lower, higher) contestant pair of `repeat` with its
/// similarity, read from every contestant's own `evals.json["cosine"]["peers"]`.
/// `None` means no contestant's saved file has a `cosine` entry for this
/// repeat at all; `Some(vec![])` means at least one does, but none of them
/// has a numeric peer (an error, a skip, or a lone participant).
fn load_repeat(meta: &Path, contestants: usize, repeat: u32) -> Option<Vec<(usize, usize, f32)>> {
    let mut out = Vec::new();
    let mut any_cosine = false;
    for c in 0..contestants {
        let path = meta
            .join(c.to_string())
            .join(repeat.to_string())
            .join("evals.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let Some(cosine_value) = value.get("cosine") else {
            continue;
        };
        any_cosine = true;
        let Some(peers) = cosine_value["peers"].as_object() else {
            continue;
        };
        for (other, sim) in peers {
            let Ok(other) = other.parse::<usize>() else {
                continue;
            };
            let Some(sim) = sim.as_f64() else { continue };
            if other > c {
                out.push((c, other, sim as f32));
            }
        }
    }
    any_cosine.then_some(out)
}

/// The judge's blind-label mapping for `repeat`, if it scored one, from
/// `judge/<repeat>/judge.json` (written by [`crate::run::results::write_judge`]).
fn judge_labels(meta: &Path, repeat: u32) -> Option<BTreeMap<usize, char>> {
    let path = meta
        .join("judge")
        .join(repeat.to_string())
        .join("judge.json");
    let text = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let labels = value["labels"].as_object()?;
    let mut map = BTreeMap::new();
    for (label, contestant) in labels {
        let contestant = contestant.as_u64()? as usize;
        let label = label.chars().next()?;
        map.insert(contestant, label);
    }
    Some(map)
}
