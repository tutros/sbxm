//! `sbxm status [--watch]` (decision 171, issue 90): a dashboard over what sbxm already tracks.
//! `tasks` is the Tasks section (read-only, never calls `sbx`); the Comparison runs and Sandboxes
//! sections (decision 171(b), (e)) are separate issues.

pub mod tasks;
