//! The screen is best-effort (issue 667, answer 2): a write to stdout or stderr that fails (a
//! broken pipe, a closed console) is dropped and never fails the command. A child of a killed
//! `task loop`, or a command whose reader went away, finishes its work and ends with its true
//! outcome. Records (`task.json`, `outcome.json`, `run.log`, the loop's logs) are files written
//! with their own errors, never through this.

use std::io::{self, Write};

/// A writer to the screen whose failures are dropped.
pub struct Screen<W: Write>(W);

impl<W: Write> Screen<W> {
    pub fn new(inner: W) -> Self {
        Self(inner)
    }
}

/// The process's stdout, best-effort.
pub fn stdout() -> Screen<io::Stdout> {
    Screen::new(io::stdout())
}

/// The process's stderr, best-effort.
pub fn stderr() -> Screen<io::Stderr> {
    Screen::new(io::stderr())
}

/// `text` on stdout, best-effort: `print!` without its panic.
pub fn print(text: &str) {
    let _ = stdout().write_all(text.as_bytes());
}

/// `text` and a newline on stdout, best-effort: `println!` without its panic.
pub fn println(text: &str) {
    print(&format!("{text}\n"));
}

/// `text` on stderr, best-effort: `eprint!` without its panic.
pub fn eprint(text: &str) {
    let _ = stderr().write_all(text.as_bytes());
}

/// `text` and a newline on stderr, best-effort: `eprintln!` without its panic.
pub fn eprintln(text: &str) {
    eprint(&format!("{text}\n"));
}

impl<W: Write> Write for Screen<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.0.write(buf) {
            Ok(n) if n > 0 || buf.is_empty() => Ok(n),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => Err(e),
            // Gone, or taking nothing: the text is dropped.
            Ok(_) | Err(_) => Ok(buf.len()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        let _ = self.0.flush();
        Ok(())
    }
}
