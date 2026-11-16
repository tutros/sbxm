//! A process told apart from a later one with the same pid (decision 163): its pid and start
//! time, and the probe that looks a pid up.

use serde::{Deserialize, Serialize};

use crate::run::results::format_timestamp;

/// An `sbxm` process (the one working on a task, or holding a lock): its pid and when it started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Process {
    pub pid: u32,
    pub started_at: String,
}

impl Process {
    /// `started_secs`: the process's start time in seconds since the epoch.
    pub fn new(pid: u32, started_secs: u64) -> Self {
        Self {
            pid,
            started_at: format_timestamp(started_secs),
        }
    }

    /// Whether this process is still running: its pid with the same start time, so a reused pid
    /// doesn't count (decision 163).
    pub fn alive(&self, probe: &dyn ProcessProbe) -> bool {
        probe
            .start_time(self.pid)
            .is_some_and(|started| format_timestamp(started) == self.started_at)
    }

    /// This process, as the probe sees it.
    pub fn current(probe: &dyn ProcessProbe) -> Self {
        let pid = std::process::id();
        Self::new(pid, probe.start_time(pid).unwrap_or(0))
    }
}

/// Looks a process up by pid (faked in tests).
pub trait ProcessProbe: Send + Sync {
    /// The start time (seconds since the epoch) of the process with this pid, if it exists.
    fn start_time(&self, pid: u32) -> Option<u64>;
}

/// The real probe, over the operating system's process table.
pub struct SystemProbe;

impl ProcessProbe for SystemProbe {
    fn start_time(&self, pid: u32) -> Option<u64> {
        use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
        let pid = Pid::from_u32(pid);
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing(),
        );
        system.process(pid).map(sysinfo::Process::start_time)
    }
}
