//! The loop's retro alert (issue 714, decision 330(4)): at an idle wait, at most once an hour
//! ([`retro::CHECK_EVERY`]), the loop works out whether a retro is due ([`retro::check`]). The
//! first time it sees one due it comments once on the open `loop-log` issue; while it stays due it
//! shows one toast a day (the key names the day, so [`Shown`](crate::notify::Shown) keeps it to
//! one across restarts) and records the line `sbxm status` shows in its lock. A failed read is one
//! warning line, and one line in the loop's own log, and the check waits for the next hour; the loop
//! goes on.

use std::io::Write;

use anyhow::Result;

use super::{Loop, LoopLock, first_line};
use crate::notify::Notification;
use crate::task::loop_log;
use crate::task::queue;
use crate::task::retro;

impl Loop<'_> {
    pub(super) fn check_retro(
        &mut self,
        lock: &mut LoopLock,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<()> {
        let now = self.now();
        if self
            .retro_checked
            .is_some_and(|at| now.saturating_sub(at) < retro::CHECK_EVERY)
        {
            return Ok(());
        }
        self.retro_checked = Some(now);
        let due = match retro::check(self.github, &self.repo) {
            Ok(due) => due,
            Err(e) => {
                let line = format!(
                    "the retro check could not read GitHub ({}); it is tried again in an hour, \
                     the loop goes on",
                    first_line(&format!("{e:#}"))
                );
                self.retro_note(&line, warn)?;
                return Ok(());
            }
        };
        if let Err(e) = lock.retro(due.as_ref().map(retro::Due::status_line)) {
            self.warn_once(
                warn,
                format!(
                    "the retro alert could not be written into the loop's lock ({e:#}); `sbxm \
                     status` may show it behind"
                ),
            )?;
        }
        let Some(due) = due else {
            return Ok(());
        };
        if !self.retro_commented.contains(due.about()) {
            let posted = loop_log::comment_once(
                self.github,
                &self.repo,
                &due.comment(),
                |body| due.commented_in(body),
                now,
            );
            match posted {
                Ok(_) => {
                    self.retro_commented.insert(due.about().to_owned());
                }
                Err(e) => {
                    let line = format!(
                        "the retro alert could not be commented on the loop-log issue ({}); it \
                         is tried again in an hour, the loop goes on",
                        first_line(&format!("{e:#}"))
                    );
                    self.retro_note(&line, warn)?;
                }
            }
        }
        let note = Notification {
            key: due.key(now),
            title: "Retro due".to_owned(),
            body: format!(
                "{} Next: run a retro (the sdlc-retro skill).",
                due.comment()
            ),
            click: self.log_click(),
        };
        self.notify(note, out, warn)
    }

    /// A failure of the retro alert, as the issue has it (issue 714, review M-1): one warning, and
    /// one line in the loop's own log ([`queue::append_note`]). A log that can't be written is a
    /// warning once; the loop goes on.
    fn retro_note(&mut self, line: &str, warn: &mut dyn Write) -> Result<()> {
        writeln!(warn, "warning: {line}")?;
        if let Err(e) = queue::append_note(&self.log, &self.repo, line) {
            self.warn_once(
                warn,
                format!("{e:#}; the loop goes on without that line in its log"),
            )?;
        }
        Ok(())
    }
}
