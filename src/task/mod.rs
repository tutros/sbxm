//! `sbxm task` (M2b): carrying a GitHub issue or PR through worker, gates,
//! review and hand-off. One module per concern, listed in sdlc/spec-m2b.md §1.

pub mod config;
pub mod findings;
pub mod finish;
pub mod gates;
pub mod pipeline;
pub mod prompts;
pub mod record;
pub mod repo;
pub mod review;
pub mod select;
