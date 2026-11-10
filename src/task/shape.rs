//! The billable shapes of a cloud sandbox (issue 559). `--cpus` and `--memory` pick the shape and
//! only these five are accepted (M4 spike P2, `sdlc/spikes/m4-p2-capture.md` item 5; the refusal
//! is `src/backend/fixtures/sbx-cloud-create-shape-refused.txt`). A cloud task's sandboxes take
//! `[sandbox] cloud_shape` from `sbxm-task.toml`, else the local cpus/memory when they are exactly
//! one of the five; never a hidden default. If Docker changes the list, `sbx` still refuses at
//! create time with its own message.

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

    pub fn name(self) -> &'static str {
        self.row().1
    }

    pub fn cpus(self) -> u32 {
        self.row().2
    }

    /// The `--memory` value, in whole GiB (`16g`), the form the spike checked.
    pub fn memory(self) -> String {
        format!("{}g", self.row().3 / 1024)
    }
}
