pub mod config_init;
pub mod config_show;
pub mod list;
pub mod new;
pub mod open;
pub mod rm;
pub mod stop;

/// Only Claude until `--harness` arrives (milestone 1, slice 18).
pub(crate) const HARNESS: &str = "claude";
