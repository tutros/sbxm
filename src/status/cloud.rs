//! The running cloud sandboxes of `sbxm status` and `sbxm doctor` (milestone 4 slice 8, decision
//! 259): every sandbox `sbx --cloud ls --json` lists whose name starts with `sbxm-`, with its
//! shape and time left. A cloud that can't be read is one line, never a failure (P17 (a)).

use crate::backend::{Location, SandboxBackend, SandboxDetail};
use crate::run::results::parse_timestamp;
use crate::status::tasks::{display_width, format_duration, pad, truncate};

/// Only sandboxes with this prefix are sbxm's (`sbxm-<project>-<harness>`, `sbxm-task-...`).
const PREFIX: &str = "sbxm-";

/// A missing value (a shape or expiry the cloud didn't report).
const DASH: &str = "-";

/// What the cloud's list said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cloud {
    /// sbxm's sandboxes, in the cloud's order.
    Sandboxes(Vec<SandboxDetail>),
    /// The first line of the error the list failed with.
    Unreadable(String),
}

/// Asks the cloud instance of `backend` for its sandboxes once, keeping sbxm's.
pub fn read(backend: &dyn SandboxBackend) -> Cloud {
    match backend.located(Location::Cloud).list_detailed() {
        Ok(sandboxes) => Cloud::Sandboxes(
            sandboxes
                .into_iter()
                .filter(|s| s.name.starts_with(PREFIX))
                .collect(),
        ),
        Err(err) => Cloud::Unreadable(
            format!("{err:#}")
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned(),
        ),
    }
}

/// The one line an unreadable cloud prints, without its newline.
pub fn unreadable_line(error: &str) -> String {
    format!("Cloud sandboxes couldn't be read (`sbx --cloud ls --json`): {error}")
}

/// `2 CPUs, 4 GiB`, `1 CPU, 1536 MiB`; `-` when either is missing.
pub fn shape(sandbox: &SandboxDetail) -> String {
    let (Some(cpus), Some(mib)) = (sandbox.cpus, sandbox.memory_mib) else {
        return DASH.to_owned();
    };
    let cpus = if cpus == 1 {
        "1 CPU".to_owned()
    } else {
        format!("{cpus} CPUs")
    };
    let memory = if mib % 1024 == 0 {
        format!("{} GiB", mib / 1024)
    } else {
        format!("{mib} MiB")
    };
    format!("{cpus}, {memory}")
}

/// The time until `expires_at` at `now`, `expiring` once it is reached or past (never a negative
/// time), `-` when there is none or it can't be read. The cloud writes fractional seconds
/// (`...21.604457Z`), which are dropped.
pub fn time_left(sandbox: &SandboxDetail, now: u64) -> String {
    let Some(at) = sandbox.expires_at.as_deref().and_then(expiry) else {
        return DASH.to_owned();
    };
    if at <= now {
        "expiring".to_owned()
    } else {
        format_duration(at - now)
    }
}

fn expiry(text: &str) -> Option<u64> {
    match text.split_once('.') {
        Some((whole, fraction)) => {
            let digits = fraction.strip_suffix('Z')?;
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            parse_timestamp(&format!("{whole}Z"))
        }
        None => parse_timestamp(text),
    }
}

const HEADER: [&str; 4] = ["CLOUD SANDBOX", "STATUS", "SHAPE", "LEFT"];

/// The section, after a blank line: a table of sbxm's cloud sandboxes, the one unreadable line,
/// or nothing at all for an empty list. Each line is cut to `width` on a terminal (`None`: shown
/// in full), like the Tasks table.
pub fn render(cloud: &Cloud, now: u64, width: Option<usize>) -> String {
    let lines = match cloud {
        Cloud::Sandboxes(sandboxes) if sandboxes.is_empty() => return String::new(),
        Cloud::Unreadable(error) => vec![unreadable_line(error)],
        Cloud::Sandboxes(sandboxes) => {
            let data: Vec<[String; 4]> = sandboxes
                .iter()
                .map(|s| {
                    [
                        s.name.clone(),
                        s.status.clone(),
                        shape(s),
                        time_left(s, now),
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
