pub mod checks;
pub mod config;
pub mod diff;
pub mod id;
pub mod kits;
pub mod log;
pub mod orchestrate;
pub mod preflight;
pub mod results;

/// The part of a run's evaluator sandbox names where a contestant's names have its index.
pub const JUDGE: &str = "judge";
pub const JEV: &str = "jev";

/// The sandbox of one part of run `run_id` in one repeat: `sbxm-run-<id>-<part>-<repeat>`, the
/// part a contestant's index, [`JUDGE`] or [`JEV`]. Every run sandbox is named here, so
/// [`names_sandbox_of`] (`sbxm status`'s owner) knows them all.
pub fn sandbox_name(run_id: &str, part: &str, repeat: u32) -> String {
    format!("sbxm-run-{run_id}-{part}-{repeat}")
}

/// Whether `name` is one [`sandbox_name`] makes for run `run_id`.
pub fn names_sandbox_of(name: &str, run_id: &str) -> bool {
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    name.strip_prefix("sbxm-run-")
        .and_then(|rest| rest.strip_prefix(run_id))
        .and_then(|rest| rest.strip_prefix('-'))
        .and_then(|rest| rest.split_once('-'))
        .is_some_and(|(part, repeat)| {
            (digits(part) || part == JUDGE || part == JEV) && digits(repeat)
        })
}
