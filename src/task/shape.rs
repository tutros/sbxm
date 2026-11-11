//! The billable shapes of a cloud sandbox (issue 559). `--cpus` and `--memory` pick the shape and
//! only these five are accepted (M4 spike P2, `sdlc/spikes/m4-p2-capture.md` item 5; the refusal
//! is `src/backend/fixtures/sbx-cloud-create-shape-refused.txt`). A cloud task's sandboxes take
//! `[sandbox] cloud_shape` from `sbxm-task.toml`, else the local cpus/memory when they are exactly
//! one of the five; never a hidden default. If Docker changes the list, `sbx` still refuses at
//! create time with its own message.

use anyhow::{Result, bail};

use crate::backend::Location;
use crate::config::Resources;
use crate::run::kits::Overrides;
use crate::task::config::Sandbox;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Micro,
    Small,
    Medium,
    Large,
    Xl,
}

/// Every shape with its name, vCPUs and MiB, smallest first.
const TABLE: [(Shape, &str, u32, u64); 5] = [
    (Shape::Micro, "micro", 1, 2048),
    (Shape::Small, "small", 2, 4096),
    (Shape::Medium, "medium", 4, 8192),
    (Shape::Large, "large", 8, 16384),
    (Shape::Xl, "xl", 16, 32768),
];

/// The five names, for a refusal.
pub const NAMES: &str = "micro, small, medium, large or xl";

impl Shape {
    /// The shape named `name`, matched case-insensitively (`Large` is large).
    pub fn parse(name: &str) -> Option<Shape> {
        TABLE
            .iter()
            .find(|(_, n, _, _)| n.eq_ignore_ascii_case(name))
            .map(|(shape, _, _, _)| *shape)
    }

    fn row(self) -> &'static (Shape, &'static str, u32, u64) {
        TABLE
            .iter()
            .find(|(shape, _, _, _)| *shape == self)
            .unwrap()
    }

    pub fn cpus(self) -> u32 {
        self.row().2
    }

    /// The `--memory` value, in whole GiB (`16g`), the form the spike checked.
    pub fn memory(self) -> String {
        format!("{}g", self.row().3 / 1024)
    }

    /// The shape `cpus` and `memory` are exactly (`8192m` and `8g` are both 8 GiB), if any.
    pub fn of(cpus: u32, memory: &str) -> Option<Shape> {
        let mib = memory_mib(memory)?;
        TABLE
            .iter()
            .find(|(_, _, c, m)| *c == cpus && *m == mib)
            .map(|(shape, _, _, _)| *shape)
    }
}

/// The MiB a `--memory` value names: a whole number with an optional `k`, `m`, `g` or `t`
/// suffix, any case, and an optional `b` after it (`8g`, `8192m`, `8GB`); a bare number is
/// bytes. `None` when it can't be read or isn't a whole number of MiB.
fn memory_mib(memory: &str) -> Option<u64> {
    let text = memory.trim().to_ascii_lowercase();
    let text = text.strip_suffix('b').unwrap_or(&text);
    let (digits, unit) = match text.char_indices().last()? {
        (i, 'k') => (&text[..i], 1u64 << 10),
        (i, 'm') => (&text[..i], 1 << 20),
        (i, 'g') => (&text[..i], 1 << 30),
        (i, 't') => (&text[..i], 1 << 40),
        _ => (text, 1),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let bytes = digits.parse::<u64>().ok()?.checked_mul(unit)?;
    (bytes % (1 << 20) == 0).then_some(bytes >> 20)
}

/// The cpus and memory a task's sandbox at `location` is created with: locally the `[sandbox]`
/// overrides (the global `[resources]` fill in the rest), in the cloud `[sandbox] cloud_shape`,
/// else the local values resolved against `global` when they are exactly a shape. Refused
/// otherwise, naming the key: called before anything of the task is written or created.
pub fn overrides(sandbox: &Sandbox, global: &Resources, location: Location) -> Result<Overrides> {
    if location == Location::Local {
        return Ok(Overrides {
            cpus: sandbox.cpus,
            memory: sandbox.memory.clone(),
        });
    }
    let cpus = sandbox.cpus.unwrap_or(global.cpus);
    let memory = sandbox.memory.as_deref().unwrap_or(&global.memory);
    let Some(shape) = sandbox.cloud_shape.or_else(|| Shape::of(cpus, memory)) else {
        let cpus = match cpus {
            1 => "1 CPU".to_owned(),
            n => format!("{n} CPUs"),
        };
        bail!(
            "{cpus} / {memory} isn't a cloud shape; set [sandbox] cloud_shape in sbxm-task.toml \
             to {NAMES}"
        );
    };
    Ok(Overrides {
        cpus: Some(shape.cpus()),
        memory: Some(shape.memory()),
    })
}
