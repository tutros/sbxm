//! `sbxm status [--watch]` (decision 171, issue 90): a dashboard over what sbxm already tracks.
//! `tasks` is the Tasks section (read-only, never calls `sbx`); `cloud` is the running cloud
//! sandboxes (`sbx --cloud ls`, milestone 4 slice 8), which `doctor` lists too; `runs` is the
//! Comparison runs section (issue 91); `looping` is the Loop section (decision 294); the Sandboxes
//! section (decision 171(b), (e)) is a separate issue.

pub mod cloud;
pub mod looping;
pub mod runs;
pub mod tasks;
