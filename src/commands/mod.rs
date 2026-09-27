pub mod config_init;
pub mod config_show;
pub mod doctor;
pub mod list;
pub mod new;
pub mod open;
pub mod rm;
pub mod stop;

use crate::harness::Harness;

/// The `--harness` argument selecting `harness` in a hint, empty for the
/// default, so Claude-only users see the commands they already know.
fn harness_flag(harness: Harness) -> String {
    if harness == Harness::default() {
        String::new()
    } else {
        format!(" --harness {}", harness.as_str())
    }
}

/// `harness` followed by a space for messages, empty for the default.
fn harness_label(harness: Harness) -> String {
    if harness == Harness::default() {
        String::new()
    } else {
        format!("{} ", harness.as_str())
    }
}
