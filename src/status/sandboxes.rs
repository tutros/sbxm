//! The Sandboxes section of `sbxm status` (decision 171(b), issue 92): every local sandbox in
//! `sbx ls --json`, with its owner, its state and its age. The owner comes from the name and the
//! host-side state files: a task folder (`sbxm-task-<id>-...`), a run folder (any name
//! [`crate::run::sandbox_name`] makes: a contestant's, the judge's or Jev's), a project's
//! `state.json`, else an orphan. The cloud sandboxes keep their own list ([`super::cloud`]). A
//! list that can't be read is one line, never a failure.
use std::fs;
use std::path::Path;

use crate::backend::{SandboxBackend, SandboxDetail};
use crate::run;
use crate::status::cloud::instant;
use crate::status::tasks::{display_width, format_duration, pad, truncate};
use crate::task::record;

/// A missing value (an age `sbx` didn't report).
const DASH: &str = "-";

/// What the local list said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Local {
    /// Every local sandbox, in `sbx`'s order.
    Sandboxes(Vec<SandboxDetail>),
    /// The first line of the error the list failed with.
    Unreadable(String),
}

/// Asks `backend`, the local instance `status` is given, for its sandboxes once.
pub fn read(backend: &dyn SandboxBackend) -> Local {
    match backend.list_with_created_at() {
        Ok(sandboxes) => Local::Sandboxes(sandboxes),
        Err(err) => Local::Unreadable(
            format!("{err:#}")
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned(),
        ),
    }
}

/// Who a sandbox belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Owner {
    Task(String),
    Run(String),
    Project(String),
    /// No task, run or project of this base dir claims it.
    Orphan,
}

impl Owner {
    /// The owner as its cell shows it: a task with its title (issue 246), from `claims`.
    fn cell(&self, claims: &Claims) -> String {
        match self {
            Owner::Task(id) => crate::title::named(
                &format!("task {id}"),
                claims.titles.get(id).map_or("", String::as_str),
            ),
            Owner::Run(id) => format!("run {id}"),
            Owner::Project(name) => format!("project {name}"),
            Owner::Orphan => "orphan".to_owned(),
        }
    }
}

/// What the host-side state files under `<base>/.sbxm/` claim: read once per frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Claims {
    /// The task ids with a `task.json`.
    tasks: Vec<String>,
    /// Each task's title, by id, from its `task.json` (issue 246); none for one unreadable.
    titles: std::collections::BTreeMap<String, String>,
    /// The run ids with a folder under `runs/`.
    runs: Vec<String>,
    /// `(sandbox, project)` from each project's `state.json`.
    projects: Vec<(String, String)>,
}

/// The claims of `base`'s state files. A file or folder that can't be read claims nothing.
pub fn claims(base: &Path) -> Claims {
    let root = base.join(".sbxm");
    let tasks: Vec<String> = record::task_folders(base)
        .unwrap_or_default()
        .into_iter()
        .filter(|dir| dir.join("task.json").is_file())
        .filter_map(|dir| Some(dir.file_name()?.to_str()?.to_owned()))
        .collect();
    let titles = tasks
        .iter()
        .filter_map(|id| {
            let rec = record::read(&record::task_dir(base, id).join("task.json")).ok()?;
            Some((id.clone(), rec.title))
        })
        .collect();
    let runs = folder_names(&root.join("runs"));
    let projects = folder_names(&root)
        .into_iter()
        .filter_map(|project| {
            let state = crate::state::load(&root.join(&project)).ok()??;
            Some(
                state
                    .sandboxes
                    .into_values()
                    .map(|s| (s.sandbox, project.clone()))
                    .collect::<Vec<_>>(),
            )
        })
        .flatten()
        .collect();
    Claims {
        tasks,
        titles,
        runs,
        projects,
    }
}

fn folder_names(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            entry.path().is_dir().then_some(())?;
            entry.file_name().to_str().map(str::to_owned)
        })
        .collect()
}

/// The owner of the sandbox `name`: a project's `state.json` naming it, else the task or run
/// whose id its name carries (the longest such id, so `spec-a-b`'s sandbox isn't `spec-a`'s),
/// else an orphan.
pub fn owner(claims: &Claims, name: &str) -> Owner {
    if let Some((_, project)) = claims.projects.iter().find(|(s, _)| s == name) {
        return Owner::Project(project.clone());
    }
    let task = claims
        .tasks
        .iter()
        .filter(|id| {
            name.strip_prefix("sbxm-task-")
                .and_then(|rest| rest.strip_prefix(id.as_str()))
                .is_some_and(|rest| rest.len() > 1 && rest.starts_with('-'))
        })
        .max_by_key(|id| id.len());
    if let Some(id) = task {
        return Owner::Task(id.clone());
    }
    let run = claims
        .runs
        .iter()
        .find(|id| run::names_sandbox_of(name, id));
    match run {
        Some(id) => Owner::Run(id.clone()),
        None => Owner::Orphan,
    }
}

/// A sandbox's age at `now`: `now` minus its `created_at`; `None` when `sbx` didn't report one,
/// or it can't be read. A `created_at` after `now` (another clock) is 0.
pub fn age(sandbox: &SandboxDetail, now: u64) -> Option<u64> {
    let created = instant(sandbox.created_at.as_deref()?)?;
    Some(now.saturating_sub(created))
}

const HEADER: [&str; 4] = ["SANDBOX", "OWNER", "STATE", "AGE"];

/// The section, after a blank line: a table of the local sandboxes, a line saying there are none,
/// or the one unreadable line. Each line is cut to `width` on a terminal (`None`: shown in full),
/// like the Tasks table.
pub fn render(local: &Local, claims: &Claims, now: u64, width: Option<usize>) -> String {
    let lines: Vec<String> = match local {
        Local::Sandboxes(sandboxes) if sandboxes.is_empty() => {
            vec!["No local sandboxes.".to_owned()]
        }
        Local::Unreadable(error) => {
            vec![format!(
                "Local sandboxes couldn't be read (`sbx ls --json`): {error}"
            )]
        }
        Local::Sandboxes(sandboxes) => {
            let data: Vec<[String; 4]> = sandboxes
                .iter()
                .map(|s| {
                    [
                        s.name.clone(),
                        owner(claims, &s.name).cell(claims),
                        s.status.clone(),
                        age(s, now).map_or_else(|| DASH.to_owned(), format_duration),
                    ]
                })
                .collect();
            let mut widths = HEADER.map(display_width);
            for row in &data {
                for (width, cell) in widths.iter_mut().zip(row) {
                    *width = (*width).max(display_width(cell));
                }
            }
            std::iter::once(HEADER.map(str::to_owned))
                .chain(data)
                .map(|cells| {
                    let mut line = String::new();
                    for (cell, width) in cells[..3].iter().zip(&widths) {
                        line.push_str(&pad(cell, *width));
                        line.push_str("  ");
                    }
                    line + &cells[3]
                })
                .collect()
        }
    };
    let mut out = String::from("\n");
    for line in lines {
        match width {
            Some(width) => out.push_str(&truncate(&line, width)),
            None => out.push_str(&line),
        }
        out.push('\n');
    }
    out
}
