//! `sbxm template rm <profile>` (decision 205(b)).

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::backend::SandboxBackend;
use crate::config::{self, GlobalConfig};
use crate::harness::Harness;
use crate::template;

#[derive(Debug, Default)]
pub struct Options {
    pub profile: String,
    pub harness: Harness,
}

/// Removes every tag the record of `<profile>-<harness>` names, its previous tags included, then
/// the record. Refused while a sandbox or task records any of them, because `sbx template rm
/// --force` doesn't refuse (decision 205(b)). A failed removal keeps the record, so a rerun
/// retries; an image already gone is skipped.
pub fn run(
    config_dir: &Path,
    options: &Options,
    backend: &dyn SandboxBackend,
    out: &mut dyn Write,
) -> Result<()> {
    config::validate_profile_name(&options.profile)?;
    let config = GlobalConfig::load(config_dir)?;
    let key = template::key(&options.profile, options.harness);
    // A build of the key changes its record and images too (decision 212); `rm` doesn't race it.
    let lock = template::dir(&config.base_dir).join(format!("{key}.build.json"));
    if lock.exists() {
        bail!(
            "a template build of {key} is running or was interrupted ({}); wait for it, or delete \
             that file if no build is running",
            lock.display()
        );
    }
    let Some(record) = template::Record::load(&config.base_dir, &key)? else {
        bail!(
            "profile '{}' has no template for {}; `sbxm template ls` lists the templates",
            options.profile,
            options.harness.as_str()
        );
    };
    let tags: Vec<&String> = record.previous.iter().chain([&record.tag]).collect();
    for tag in &tags {
        let users = template::users(&config.base_dir, tag)?;
        if !users.is_empty() {
            bail!(
                "template {tag} is in use by {}; remove those first",
                users.join(", ")
            );
        }
    }
    let images = backend.template_ls()?;
    for tag in tags {
        if !images.iter().any(|t| t.reference() == *tag) {
            continue;
        }
        backend
            .template_rm(tag)
            .with_context(|| format!("could not remove template {tag}; run it again"))?;
        writeln!(out, "removed template {tag}")?;
    }
    let path = template::dir(&config.base_dir).join(format!("{key}.json"));
    fs::remove_file(&path).with_context(|| format!("cannot remove {}", path.display()))
}
