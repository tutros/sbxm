//! `sbxm task` (M2b): carrying a GitHub issue or PR through worker, gates,
//! review and hand-off. One module per concern, listed in sdlc/spec-m2b.md §1.

pub mod closes;
pub mod config;
pub mod findings;
pub mod finish;
pub mod fixes;
pub mod gates;
pub mod jobs;
pub mod lifetime;
pub mod lock;
pub mod loop_log;
pub mod loop_pause;
pub mod machine;
pub mod medium;
pub mod merged;
pub mod next;
pub mod pipeline;
pub mod pr_fix;
pub mod prompts;
pub mod queue;
pub mod real_check;
pub mod record;
pub mod repo;
pub mod resume;
pub mod review;
pub mod rewrite;
pub mod risk;
pub mod runlog;
pub mod secrets;
pub mod select;
pub mod session;
pub mod stale_lock;
pub mod summary;
