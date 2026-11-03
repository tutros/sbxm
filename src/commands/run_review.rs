//! `sbxm run review <run-id> [--submit | --restart]` (issue 347, decisions 18, 21, 22): human
//! review of a saved run, in two steps. The plain command writes `<meta>/review/`, one
//! `repeat-<n>/` per repeat with each contestant's answer and diff under a blind label of its
//! own and a blank `scores.toml`; `--submit` checks every `scores.toml` and only then saves the
//! scores in each pair's `evals.json` under `human`, revealing the labels. The id is validated
//! before any path is built from it, and every input is read before the first write.

use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

use crate::config::GlobalConfig;
use crate::confirm::Confirm;
use crate::eval::{human, score};
use crate::run::config::RunConfig;
use crate::run::{id, results};
use crate::task::review::agent_label;

/// The review folder's name inside the run's metadata folder.
const REVIEW_DIR: &str = "review";

/// A saved, completed run with a rubric: what both steps start from.
struct Run {
    id: String,
    meta: PathBuf,
    config: RunConfig,
}

impl Run {
    fn contestants(&self) -> Vec<usize> {
        (0..self.config.contestants.len()).collect()
    }

    fn review(&self) -> PathBuf {
        self.meta.join(REVIEW_DIR)
    }

    fn repeat_dir(&self, repeat: u32) -> PathBuf {
        self.review().join(format!("repeat-{}", repeat + 1))
    }

    fn pair_dir(&self, contestant: usize, repeat: u32) -> PathBuf {
        self.meta
            .join(contestant.to_string())
            .join(repeat.to_string())
    }
}

fn open_run(config_dir: &Path, run_id: &str) -> Result<Run> {
    if !id::is_valid(run_id) {
        bail!(
            "{run_id:?} isn't a run id; run ids look like 2026-09-30-a1b2c3 (`sbxm run` prints \
             one as `Run <id>`)"
        );
    }
    let global = GlobalConfig::load(config_dir)?;
    let meta = global.base_dir.join(".sbxm").join("runs").join(run_id);
    if !meta.is_dir() {
        bail!(
            "no run {run_id} under {}; check the id `sbxm run` printed",
            meta.parent().unwrap_or(&meta).display()
        );
    }
    let record_path = meta.join("run.json");
    let record: Value = serde_json::from_str(
        &fs::read_to_string(&record_path)
            .with_context(|| format!("cannot read {}", record_path.display()))?,
    )
    .with_context(|| format!("{} is not valid JSON", record_path.display()))?;
    if record["completed_at"].as_str().is_none() {
        bail!(
            "run {run_id} is not completed (still running, or interrupted); review a run once \
             `sbxm run` has finished it (`sbxm run show {run_id}` says whether it has)"
        );
    }
    let config_path = meta.join("run-config.toml");
    let config = RunConfig::load(&config_path).with_context(|| {
        format!(
            "cannot read the run's config copy {}",
            config_path.display()
        )
    })?;
    if config.eval.rubric.is_empty() {
        bail!(
            "run {run_id} has no rubric to score; add [[eval.rubric]] criteria to {} to review it",
            config_path.display()
        );
    }
    Ok(Run {
        id: run_id.to_owned(),
        meta,
        config,
    })
}

/// Writes the review folder and returns its path. A folder already there is refused unless
/// `restart`, which asks (showing the path) before deleting it.
pub fn prepare(
    config_dir: &Path,
    run_id: &str,
    restart: bool,
    confirm: &dyn Confirm,
) -> Result<PathBuf> {
    let run = open_run(config_dir, run_id)?;
    let review = run.review();
    let existing = fs::symlink_metadata(&review).ok();
    if let Some(found) = &existing {
        if !restart {
            bail!(
                "run {run_id} already has a review folder, {}; fill in its scores.toml files and \
                 run `sbxm run review {run_id} --submit`, or start over with `sbxm run review \
                 {run_id} --restart` (which deletes it)",
                review.display()
            );
        }
        if !found.is_dir() || found.file_type().is_symlink() {
            bail!(
                "{} is a link or not a folder, so --restart won't delete it; move it aside \
                 yourself, then run `sbxm run review {run_id}`",
                review.display()
            );
        }
        check_inside(&run.meta, &review)?;
    }

    // Everything the folder will hold, read before anything is deleted or written.
    let files = layout(&run)?;

    if existing.is_some() {
        if !confirm.is_interactive() {
            bail!(
                "--restart deletes {} and every score in it, so it asks first and needs a \
                 terminal; run it in one",
                review.display()
            );
        }
        if !confirm.confirm(&format!(
            "Delete {} with its scores.toml files and lay the run out again?",
            review.display()
        ))? {
            bail!("kept {}; nothing was changed", review.display());
        }
        fs::remove_dir_all(&review)
            .with_context(|| format!("cannot delete {}", review.display()))?;
    }

    fs::create_dir(&review).with_context(|| format!("cannot create {}", review.display()))?;
    if let Err(e) = write_files(&review, &files) {
        // Only what this call just created goes.
        let _ = fs::remove_dir_all(&review);
        return Err(e);
    }
    Ok(review)
}

/// The review folder's files, as paths relative to it and their contents.
fn layout(run: &Run) -> Result<Vec<(PathBuf, Vec<u8>)>> {
    let repeats = run.config.run.repeat;
    let mut files = vec![(
        PathBuf::from("task.md"),
        format!("{}\n", run.config.task.prompt.trim_end()).into_bytes(),
    )];
    for repeat in 0..repeats {
        let labels = human::assign_labels(&run.id, repeat, &run.contestants());
        let repeat_dir = PathBuf::from(format!("repeat-{}", repeat + 1));
        let letters: Vec<char> = labels.iter().map(|(_, label)| *label).collect();
        files.push((
            repeat_dir.join("scores.toml"),
            human::scores_template(&run.id, repeat, repeats, &run.config.eval.rubric, &letters)
                .into_bytes(),
        ));
        for (contestant, label) in labels {
            let pair = run.pair_dir(contestant, repeat);
            let folder = repeat_dir.join(label.to_string());
            let answer = read_optional(&pair.join("answer.md"))?
                .filter(|text| !String::from_utf8_lossy(text).trim().is_empty());
            let result = read_optional(&pair.join("result.json"))?
                .and_then(|text| serde_json::from_slice::<Value>(&text).ok())
                .unwrap_or(Value::Null);
            let diff = if result["diff"]["status"] == "ok" {
                read_optional(&pair.join("diff.patch"))?
            } else {
                None
            };
            if answer.is_none() && diff.is_none() {
                files.push((
                    folder.join("no-output.md"),
                    b"This candidate left no answer and no diff (it could not run, or produced \
                      nothing). Its criteria may stay blank.\n"
                        .to_vec(),
                ));
                continue;
            }
            if let Some(answer) = answer {
                files.push((folder.join("answer.md"), answer));
            }
            if let Some(diff) = diff {
                files.push((folder.join("diff.patch"), diff));
            }
        }
    }
    Ok(files)
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
    }
}

fn write_files(root: &Path, files: &[(PathBuf, Vec<u8>)]) -> Result<()> {
    for (relative, contents) in files {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }
        fs::write(&path, contents).with_context(|| format!("cannot write {}", path.display()))?;
    }
    Ok(())
}

/// The folder about to be deleted must sit directly in the run's folder once links are resolved.
fn check_inside(meta: &Path, review: &Path) -> Result<()> {
    let parent =
        fs::canonicalize(meta).with_context(|| format!("cannot resolve {}", meta.display()))?;
    let resolved =
        fs::canonicalize(review).with_context(|| format!("cannot resolve {}", review.display()))?;
    if resolved.parent() != Some(parent.as_path()) {
        bail!(
            "{} resolves to {}, outside the run's folder, so --restart won't delete it; move it \
             aside yourself",
            review.display(),
            resolved.display()
        );
    }
    Ok(())
}

/// Checks every repeat's `scores.toml`, then saves the scores under `human` in each pair's
/// `evals.json` and returns the report: what was saved or replaced, the blanks, the labels and
/// the human ranking.
pub fn submit(config_dir: &Path, run_id: &str) -> Result<String> {
    let run = open_run(config_dir, run_id)?;
    let review = run.review();
    if !review.is_dir() {
        bail!(
            "run {run_id} has no review folder ({}); run `sbxm run review {run_id}` first, fill \
             in its scores.toml files, then --submit",
            review.display()
        );
    }
    let repeats = run.config.run.repeat;
    let rubric = &run.config.eval.rubric;

    // Every check before the first write: each file, then each pair's folder and evals.json.
    let mut entries: Vec<(PathBuf, Value)> = Vec::new();
    let mut blanks: Vec<String> = Vec::new();
    let mut mapping: Vec<String> = Vec::new();
    let mut replaced = false;
    let names: Vec<String> = run
        .config
        .contestants
        .iter()
        .map(|c| agent_label(c.harness.as_str(), Some(&c.model)))
        .collect();
    for repeat in 0..repeats {
        let labels = human::assign_labels(&run.id, repeat, &run.contestants());
        let letters: Vec<char> = labels.iter().map(|(_, label)| *label).collect();
        let path = run.repeat_dir(repeat).join("scores.toml");
        let text = fs::read_to_string(&path).with_context(|| {
            format!(
                "cannot read {}; start over with `sbxm run review {run_id} --restart` if it is \
                 gone",
                path.display()
            )
        })?;
        let scores = human::parse_scores(&path, &text, rubric, &letters)?;
        let mut shown = Vec::new();
        for (contestant, label) in &labels {
            let pair = run.pair_dir(*contestant, repeat);
            if !pair.join("result.json").is_file() {
                bail!(
                    "{} has no result.json, so the run's files are incomplete; nothing was saved",
                    pair.display()
                );
            }
            let evals_path = pair.join("evals.json");
            if let Some(bytes) = read_optional(&evals_path)? {
                let evals: Value = serde_json::from_slice(&bytes)
                    .with_context(|| format!("{} is not valid JSON", evals_path.display()))?;
                replaced |= evals.get("human").is_some();
            }
            let mine = &scores[label];
            let criteria: Map<String, Value> = mine
                .criteria
                .iter()
                .map(|(id, (value, score))| (id.clone(), json!({"value": value, "score": score})))
                .collect();
            entries.push((
                pair,
                json!({"label": label.to_string(), "criteria": criteria, "unscored": mine.unscored}),
            ));
            if !mine.unscored.is_empty() {
                blanks.push(format!(
                    "repeat {}/{repeats} {label}: {}",
                    repeat + 1,
                    mine.unscored.join(", ")
                ));
            }
            shown.push(format!(
                "{label} = contestants[{contestant}] {}",
                names.get(*contestant).map_or("?", String::as_str)
            ));
        }
        mapping.push(format!(
            "  repeat {}/{repeats}: {}",
            repeat + 1,
            shown.join(", ")
        ));
    }

    for (pair, entry) in entries {
        results::merge_evals(&pair, "human", entry)?;
    }

    let mut out = String::new();
    writeln!(
        out,
        "Saved the human scores of run {run_id} ({} contestants, {repeats} repeats).",
        names.len()
    )?;
    if replaced {
        writeln!(out, "Replaced the human scores saved earlier.")?;
    }
    if !blanks.is_empty() {
        writeln!(out, "Left blank (unscored):")?;
        for blank in &blanks {
            writeln!(out, "  {blank}")?;
        }
    }
    writeln!(out, "Labels:")?;
    for line in &mapping {
        writeln!(out, "{line}")?;
    }
    // The scores are saved by now, so a ranking that can't be computed is reported, not fatal.
    match score::load_human(&run.meta) {
        Ok(Some(ranking)) => {
            writeln!(out)?;
            write!(out, "{}", score::render_human(&ranking, &names))?;
        }
        Ok(None) => {}
        Err(e) => writeln!(out, "Cannot rank the run by human scores: {e:#}")?,
    }
    Ok(out)
}
