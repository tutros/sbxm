//! Asking the user before destructive actions.

use std::io::{self, BufRead, IsTerminal, Write};

use anyhow::Result;

pub trait Confirm {
    /// Whether a person can answer (stdin and stderr are a terminal).
    fn is_interactive(&self) -> bool;
    /// Shows `prompt` and returns whether the user said yes.
    fn confirm(&self, prompt: &str) -> Result<bool>;
}

/// Asks on the terminal; anything but `y`/`yes` means no.
#[derive(Debug, Default)]
pub struct Terminal;

impl Confirm for Terminal {
    fn is_interactive(&self) -> bool {
        io::stdin().is_terminal() && io::stderr().is_terminal()
    }

    fn confirm(&self, prompt: &str) -> Result<bool> {
        eprint!("{prompt} [y/N] ");
        io::stderr().flush()?;
        let mut line = String::new();
        io::stdin().lock().read_line(&mut line)?;
        Ok(matches!(
            line.trim().to_ascii_lowercase().as_str(),
            "y" | "yes"
        ))
    }
}
