//! `sbxm template ls` (decision 206).

use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use clap::ValueEnum;

use crate::backend::SandboxBackend;
use crate::config::GlobalConfig;
use crate::disk::gib;
use crate::harness::Harness;
use crate::run::results::parse_timestamp;
use crate::status::tasks::format_duration;
use crate::template;

#[derive(Debug, Default)]
pub struct Options {
    /// Seconds since the Unix epoch for the ages; the clock when `None`.
    pub now: Option<u64>,
}

/// One line per template record: profile, harness, tag, warm or cold, size, age and how many
/// sandboxes and tasks use any of its tags, flagging a record whose image is gone. Asks `sbx`
/// only when there is a record.
pub fn run(
    config_dir: &Path,
    options: &Options,
    backend: &dyn SandboxBackend,
    out: &mut dyn Write,
) -> Result<()> {
    let config = GlobalConfig::load(config_dir)?;
    let records = records(&config.base_dir)?;
    if records.is_empty() {
        writeln!(
            out,
            "no templates; build one with `sbxm template build <profile>`"
        )?;
        return Ok(());
    }
    let now = match options.now {
        Some(now) => now,
        None => SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
    };
    let images = backend.template_ls()?;
    for (profile, harness, record) in records {
        let users = std::iter::once(&record.tag)
            .chain(&record.previous)
            .map(|tag| template::users(&config.base_dir, tag).map(|u| u.len()))
            .sum::<Result<usize>>()
            .map_or_else(|err| format!("? ({err:#})"), |n| n.to_string());
        let age = parse_timestamp(&record.created_at)
            .map_or("?".to_owned(), |at| format_duration(now.saturating_sub(at)));
        let mut line = format!(
            "{profile}  {}  {}  {}  {}  {age} old  used by {users}",
            harness.as_str(),
            record.tag,
            if record.warm { "warm" } else { "cold" },
            gib(record.size_bytes),
        );
        if !record.previous.is_empty() {
            line += &format!("  (+{} previous)", record.previous.len());
        }
        if !images.iter().any(|t| t.reference() == record.tag) {
            let flag = match harness {
                Harness::Claude => String::new(),
                other => format!(" --harness {}", other.as_str()),
            };
            line += &format!(
                "  image missing; rebuild it with `sbxm template build {profile}{flag}` or \
                 remove the record with `sbxm template rm {profile}{flag}`"
            );
        }
        writeln!(out, "{line}")?;
    }
    Ok(())
}

/// Every `<profile>-<harness>.json` under `<base>/.sbxm/templates`, sorted by file name; build
/// locks, temporary files and folders are skipped.
fn records(base_dir: &Path) -> Result<Vec<(String, Harness, template::Record)>> {
    let dir = template::dir(base_dir);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut keys = Vec::new();
    for entry in fs::read_dir(&dir).with_context(|| format!("cannot read {}", dir.display()))? {
        let entry = entry.with_context(|| format!("cannot read {}", dir.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(key) = name.strip_suffix(".json") else {
            continue;
        };
        if key.ends_with(".build") || !entry.file_type()?.is_file() {
            continue;
        }
        let Some((profile, harness)) = key.rsplit_once('-') else {
            continue;
        };
        let Ok(harness) = Harness::from_str(harness, false) else {
            continue;
        };
        keys.push((key.to_owned(), profile.to_owned(), harness));
    }
    keys.sort_by(|a, b| a.0.cmp(&b.0));
    let mut records = Vec::new();
    for (key, profile, harness) in keys {
        if let Some(record) = template::Record::load(base_dir, &key)? {
            records.push((profile, harness, record));
        }
    }
    Ok(records)
}
