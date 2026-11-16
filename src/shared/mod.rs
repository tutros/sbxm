//! What the sandbox side and `sbxm task` both use (issue 539): leaf helpers that name neither
//! side's records. The dependency runs one way, task -> sandbox, so the sandbox side
//! (`src/backend/`, `src/slot.rs`, `src/template.rs`, `src/run/`, `src/eval/`, `src/github/`, and
//! the `run` and `template build` commands) never names `crate::task`; the `task::` modules
//! re-export what moved here, so task code keeps its paths.

pub mod files;
pub mod gates;
pub mod kind;
pub mod lock;
pub mod names;
pub mod process;
pub mod runlog;
