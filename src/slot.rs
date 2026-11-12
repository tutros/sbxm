//! The machine-wide sandbox slots (issue 504, decision 304): `[resources] sandbox_slots = N` in
//! `config.toml` lets at most N commands that create a local sandbox run on this machine at once.
//! Unset, nothing here touches the disk.
//!
//! A slot is a lock file, `slot-<k>.json` for k in 1..=N, in the per-user local data folder
//! ([`slots_dir`]), never under a base dir or the config dir, so every base dir and config dir of
//! one user on one machine shares them. The file names its holder: the process (pid and start
//! time, so a reused pid isn't mistaken for it), the command line, the task id when there is one,
//! and since when. A command holds one slot from its [`take`] until the [`Slot`] is dropped; inside
//! one thread a second [`take`] while one is held is the same slot, so a command that goes on to
//! another (`task run` to its review, `task loop` to a task's phases) never waits on itself.
//!
//! A command that finds no free slot waits: it writes a ticket in `queue/` (its name sorts by
//! arrival), looks again every [`POLL`], and takes a slot only when fewer older tickets are live
//! than slots are free, so waiting commands start in arrival order. It prints who holds the slot
//! once, and again when the holder changes. `--no-wait` ([`Gate::no_wait`]) refuses instead,
//! naming the holder, without writing anything. A slot or ticket whose process is gone is taken
//! over, a slot with one line naming the dead holder. Ctrl-C while waiting removes this process's
//! ticket (and any slot it holds) and exits. In `task loop` the wait also polls the loop's stop
//! request ([`Gate::stop`], issue 570): a stop gives up the ticket, or the slot just taken, and
//! the command fails with [`Stopped`], having created nothing.
//!
//! `[resources] cloud_slots = N` (issue 599, decision 317) limits the commands that create a
//! cloud sandbox the same way, in a pool of its own ([`Pool::Cloud`], taken with [`take_cloud`]):
//! its own folder next to the sandbox slots' ([`cloud_slots_dir`]), and its own held slot per
//! thread, so a cloud command and a local one never wait on each other.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Once, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::config::GlobalConfig;
use crate::run::results::{format_timestamp, now};
use crate::task::lock;
use crate::task::record::{Process, ProcessProbe, SystemProbe};

/// How often a waiting command looks at the slots again.
pub const POLL: Duration = Duration::from_secs(5);

/// The folder of the waiting commands' tickets, inside the slots folder.
const QUEUE: &str = "queue";

/// The two pools of slots: a command creating local sandboxes takes a sandbox slot, one creating
/// cloud sandboxes a cloud slot, never both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pool {
    /// `sandbox_slots` (decision 304).
    Local,
    /// `cloud_slots` (issue 599, decision 317).
    Cloud,
}

impl Pool {
    /// The `[resources]` setting that sizes it.
    fn setting(self) -> &'static str {
        match self {
            Pool::Local => "sandbox_slots",
            Pool::Cloud => "cloud_slots",
        }
    }

    /// One of its slots, in messages.
    fn noun(self) -> &'static str {
        match self {
            Pool::Local => "sandbox slot",
            Pool::Cloud => "cloud slot",
        }
    }

    /// Its folder's name, inside `<per-user local data folder>/sbxm`.
    fn folder(self) -> &'static str {
        match self {
            Pool::Local => "slots",
            Pool::Cloud => "cloud-slots",
        }
    }

    fn count(self, config: &GlobalConfig) -> Option<u32> {
        match self {
            Pool::Local => config.resources.sandbox_slots,
            Pool::Cloud => config.resources.cloud_slots,
        }
    }

    fn real_dir(self) -> Result<PathBuf> {
        dirs::data_local_dir()
            .map(|dir| dir.join("sbxm").join(self.folder()))
            .with_context(|| {
                format!(
                    "cannot find the per-user local data folder the {}s live in; delete {} from \
                     [resources] of config.toml to run without a limit",
                    self.noun(),
                    self.setting()
                )
            })
    }
}

/// `<per-user local data folder>/sbxm/slots` (`%LOCALAPPDATA%\sbxm\slots` on Windows).
pub fn slots_dir() -> Result<PathBuf> {
    Pool::Local.real_dir()
}

/// `<per-user local data folder>/sbxm/cloud-slots`, next to [`slots_dir`].
pub fn cloud_slots_dir() -> Result<PathBuf> {
    Pool::Cloud.real_dir()
}

/// What a sandbox-creating command takes its slot with; tests replace the folder, the probe, the
/// waiting and the files' registry.
#[derive(Clone)]
pub struct Gate {
    /// The sandbox slots folder; `None` is [`slots_dir`]. The cloud slots live next to it, in
    /// `cloud-slots`.
    pub dir: Option<PathBuf>,
    /// `--no-wait`: refuse at once instead of waiting for a free slot.
    pub no_wait: bool,
    pub probe: Arc<dyn ProcessProbe>,
    /// How a waiting command waits between two looks.
    pub sleep: Arc<dyn Fn(Duration) + Send + Sync>,
    /// The command line a slot file names.
    pub command: String,
    /// Whether a wait catches Ctrl-C to remove this process's files before it exits (the real
    /// command; never in tests, which share one process).
    pub catch_ctrl_c: bool,
    /// The files this process wrote, removed on Ctrl-C.
    pub registry: Arc<Registry>,
    /// `task loop`'s stop request (issue 570): asked before the wait, at each look while waiting
    /// and once more after the slot is taken; when it says stop, the ticket and the slot are given
    /// up and [`take`] fails with [`Stopped`]. `None` for every other command.
    pub stop: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

impl Default for Gate {
    fn default() -> Self {
        Gate {
            dir: None,
            no_wait: false,
            probe: Arc::new(SystemProbe),
            sleep: Arc::new(std::thread::sleep),
            command: command_line(),
            catch_ctrl_c: false,
            registry: Arc::clone(&GLOBAL),
            stop: None,
        }
    }
}

impl Gate {
    /// The real command's gate, with `--no-wait` as given.
    pub fn real(no_wait: bool) -> Self {
        Gate {
            no_wait,
            catch_ctrl_c: true,
            ..Gate::default()
        }
    }

    fn dir(&self, pool: Pool) -> Result<PathBuf> {
        match (&self.dir, pool) {
            (Some(dir), Pool::Local) => Ok(dir.clone()),
            (Some(dir), Pool::Cloud) => Ok(dir.with_file_name(Pool::Cloud.folder())),
            (None, pool) => pool.real_dir(),
        }
    }

    /// Whether the loop this command runs in asked to stop.
    fn stop_asked(&self) -> bool {
        self.stop.as_ref().is_some_and(|stop| stop())
    }
}

/// [`take`] given up on `task loop`'s stop request (issue 570): no slot is held, no ticket is
/// left, and nothing was created.
#[derive(Debug)]
pub struct Stopped;

impl std::fmt::Display for Stopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("stopped on request while waiting for a slot; nothing was created")
    }
}

impl std::error::Error for Stopped {}

impl std::fmt::Debug for Gate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gate")
            .field("dir", &self.dir)
            .field("no_wait", &self.no_wait)
            .field("command", &self.command)
            .finish_non_exhaustive()
    }
}

/// `sbxm` and this process's arguments.
fn command_line() -> String {
    std::iter::once("sbxm".to_owned())
        .chain(std::env::args().skip(1))
        .collect::<Vec<_>>()
        .join(" ")
}

/// What a slot file and a ticket hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holder {
    #[serde(flatten)]
    pub process: Process,
    /// Tells two files of one process apart, so a file is only ever removed by what made it.
    pub token: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// When it took the slot or started waiting.
    pub since: String,
}

impl Holder {
    /// `<command> (task <id>, process <pid>, since <time>)`.
    pub fn describe(&self) -> String {
        let task = self
            .task
            .as_ref()
            .map(|id| format!("task {id}, "))
            .unwrap_or_default();
        format!(
            "{} ({task}process {}, since {})",
            self.command, self.process.pid, self.since
        )
    }
}

/// The slot and ticket files this process wrote and still owns, so a Ctrl-C can remove them
/// before the process exits ([`release_all`]).
#[derive(Debug, Default)]
pub struct Registry(Mutex<Vec<(PathBuf, String)>>);

impl Registry {
    fn add(&self, path: &Path, me: &str) {
        if let Ok(mut files) = self.0.lock() {
            files.push((path.to_owned(), me.to_owned()));
        }
    }

    fn forget(&self, path: &Path) {
        if let Ok(mut files) = self.0.lock() {
            files.retain(|(p, _)| p != path);
        }
    }

    /// Removes every file still registered that still holds what this process wrote.
    pub fn release(&self) {
        let files = match self.0.lock() {
            Ok(mut files) => std::mem::take(&mut *files),
            Err(_) => return,
        };
        for (path, me) in files {
            remove_if_mine(&path, &me);
        }
    }
}

static GLOBAL: LazyLock<Arc<Registry>> = LazyLock::new(Arc::default);

/// How many threads of this process are waiting for a slot now, for Ctrl-C's message.
static WAITING: AtomicUsize = AtomicUsize::new(0);

/// Removes this process's slot and ticket files (the real gates'): what Ctrl-C does before the
/// process exits, here or in `task loop`'s own handler.
pub fn release_all() {
    GLOBAL.release();
}

fn remove_if_mine(path: &Path, me: &str) {
    if fs::read_to_string(path).is_ok_and(|text| text == me) {
        let _ = fs::remove_file(path);
    }
}

/// A file made with `create_new` that this process owns until it is dropped.
#[derive(Debug)]
struct Owned {
    path: PathBuf,
    me: String,
    registry: Arc<Registry>,
}

impl Owned {
    fn create(path: &Path, me: &str, registry: &Arc<Registry>) -> std::io::Result<Self> {
        let mut file = lock::create_new_file(path)?;
        if let Err(e) = file.write_all(me.as_bytes()) {
            drop(file);
            let _ = fs::remove_file(path);
            return Err(e);
        }
        registry.add(path, me);
        Ok(Owned {
            path: path.to_owned(),
            me: me.to_owned(),
            registry: Arc::clone(registry),
        })
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        self.registry.forget(&self.path);
        remove_if_mine(&self.path, &self.me);
    }
}

/// A command's sandbox slot, given back when the last handle to it is dropped (on return, an
/// error or a panic). Empty when `sandbox_slots` is unset.
#[derive(Debug)]
pub struct Slot(Option<Arc<Owned>>);

impl Slot {
    /// No slot: for a command that takes none (a cloud task, no limit set).
    pub fn none() -> Self {
        Slot(None)
    }

    /// The slot file this handle keeps, if any.
    pub fn path(&self) -> Option<&Path> {
        self.0.as_deref().map(|owned| owned.path.as_path())
    }
}

thread_local! {
    /// The slot of each pool this thread's command holds, so a nested take is the same slot.
    static HELD: RefCell<[Weak<Owned>; 2]> = const { RefCell::new([Weak::new(), Weak::new()]) };
    /// The slots this thread took while a [`Hold`] lives, and how many holds are open.
    static KEPT: RefCell<(usize, Vec<Arc<Owned>>)> = const { RefCell::new((0, Vec::new())) };
}

/// While it lives, every slot this thread takes stays held, also once the step that took it has
/// ended (issue 599). A command made of steps that each check their own inputs before taking the
/// slot (`task run`: a start, then a review) holds one, so its first step's slot carries on
/// through the rest, and no other command takes it in between.
#[derive(Debug)]
pub struct Hold(());

/// Opens a [`Hold`].
pub fn hold() -> Hold {
    KEPT.with(|kept| kept.borrow_mut().0 += 1);
    Hold(())
}

impl Drop for Hold {
    fn drop(&mut self) {
        let given_back = KEPT.with(|kept| {
            let mut kept = kept.borrow_mut();
            kept.0 -= 1;
            match kept.0 {
                0 => std::mem::take(&mut kept.1),
                _ => Vec::new(),
            }
        });
        drop(given_back);
    }
}

fn held_index(pool: Pool) -> usize {
    match pool {
        Pool::Local => 0,
        Pool::Cloud => 1,
    }
}

/// Takes a sandbox slot for the command `gate` describes, working on `task` when it has one:
/// nothing when `sandbox_slots` is unset, the slot already held when this thread holds one, a free
/// slot at once, and otherwise waits for one (or, with `--no-wait`, refuses), printing to `warn`.
pub fn take(
    config: &GlobalConfig,
    gate: &Gate,
    task: Option<&str>,
    warn: &mut dyn Write,
) -> Result<Slot> {
    take_in(Pool::Local, config, gate, task, warn)
}

/// [`take`] for a command that creates cloud sandboxes: a slot of `cloud_slots`, nothing when it
/// is unset (issue 599).
pub fn take_cloud(
    config: &GlobalConfig,
    gate: &Gate,
    task: Option<&str>,
    warn: &mut dyn Write,
) -> Result<Slot> {
    take_in(Pool::Cloud, config, gate, task, warn)
}

fn take_in(
    pool: Pool,
    config: &GlobalConfig,
    gate: &Gate,
    task: Option<&str>,
    warn: &mut dyn Write,
) -> Result<Slot> {
    let Some(slots) = pool.count(config) else {
        return Ok(Slot(None));
    };
    if let Some(held) = HELD.with(|held| held.borrow()[held_index(pool)].upgrade()) {
        return Ok(Slot(Some(held)));
    }
    if gate.stop_asked() {
        return Err(Stopped.into());
    }
    let dir = gate.dir(pool)?;
    let queue = dir.join(QUEUE);
    fs::create_dir_all(&queue).with_context(|| {
        format!(
            "cannot create the {}s folder {}",
            pool.noun(),
            queue.display()
        )
    })?;
    let me = serde_json::to_string(&Holder {
        process: Process::current(&*gate.probe),
        token: lock::token(),
        command: gate.command.clone(),
        task: task.map(str::to_owned),
        since: format_timestamp(now()),
    })?;
    let slots = Slots {
        pool,
        dir: &dir,
        count: slots,
        gate,
        me: &me,
    };
    let owned = if gate.no_wait {
        slots.take_now(warn)?
    } else {
        slots.wait(warn)?
    };
    // A stop that came in as the slot was taken: given back before anything is created.
    if gate.stop_asked() {
        drop(owned);
        return Err(Stopped.into());
    }
    let owned = Arc::new(owned);
    HELD.with(|held| held.borrow_mut()[held_index(pool)] = Arc::downgrade(&owned));
    KEPT.with(|kept| {
        let mut kept = kept.borrow_mut();
        if kept.0 > 0 {
            kept.1.push(Arc::clone(&owned));
        }
    });
    Ok(Slot(Some(owned)))
}

/// What one slot file says now.
enum State {
    Free,
    /// Its holder is gone; the text is what to take over.
    Stale(Holder, String),
    Held(Holder),
    /// Not readable as a holder (being written, or left half-written): its text.
    Unreadable(String),
}

impl State {
    fn is_free(&self) -> bool {
        matches!(self, State::Free | State::Stale(..))
    }
}

/// The slots of one folder, as one command sees them.
struct Slots<'a> {
    pool: Pool,
    dir: &'a Path,
    count: u32,
    gate: &'a Gate,
    /// The holder this command writes.
    me: &'a str,
}

impl Slots<'_> {
    fn path(&self, k: u32) -> PathBuf {
        self.dir.join(format!("slot-{k}.json"))
    }

    fn state(&self, k: u32) -> Result<State> {
        let path = self.path(k);
        let text = match lock::read_file(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(State::Free),
            Err(e) => {
                return Err(e).with_context(|| format!("cannot read {}", path.display()));
            }
        };
        Ok(match serde_json::from_str::<Holder>(&text) {
            Ok(holder) if holder.process.alive(&*self.gate.probe) => State::Held(holder),
            Ok(holder) => State::Stale(holder, text),
            Err(_) => State::Unreadable(text),
        })
    }

    /// Every slot's state; a slot still unreadable with the very text `unreadable` held for it
    /// at the last look is left by a command that died writing it, and counts as stale.
    fn states(&self, unreadable: &mut HashMap<u32, String>) -> Result<Vec<State>> {
        let mut states = Vec::new();
        for k in 1..=self.count {
            let state = match self.state(k)? {
                State::Unreadable(text) if unreadable.get(&k) == Some(&text) => {
                    State::Stale(dead_unreadable(&self.path(k)), text)
                }
                State::Unreadable(text) => {
                    unreadable.insert(k, text.clone());
                    State::Unreadable(text)
                }
                state => {
                    unreadable.remove(&k);
                    state
                }
            };
            states.push(state);
        }
        Ok(states)
    }

    /// Tries to take slot `k`, taking over a stale holder (one line on `warn`); `None` when
    /// another command has it.
    fn try_take(&self, k: u32, state: &State, warn: &mut dyn Write) -> Result<Option<Owned>> {
        let path = self.path(k);
        if let State::Stale(_, text) = state {
            lock::take_over_stale_file(&path, text)?;
        }
        match Owned::create(&path, self.me, &self.gate.registry) {
            Ok(owned) => {
                if let State::Stale(dead, _) = state {
                    writeln!(
                        warn,
                        "took over {} {k} of {}, which is no longer running",
                        self.pool.noun(),
                        dead.describe()
                    )?;
                }
                Ok(Some(owned))
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(None),
            Err(e) => Err(e).with_context(|| format!("cannot create {}", path.display())),
        }
    }

    /// Takes the first free slot of `states`, if any.
    fn take_free(&self, states: &[State], warn: &mut dyn Write) -> Result<Option<Owned>> {
        for (k, state) in (1..).zip(states) {
            if state.is_free()
                && let Some(owned) = self.try_take(k, state, warn)?
            {
                return Ok(Some(owned));
            }
        }
        Ok(None)
    }

    /// The one admission rule, waiting or not: a free slot, but only while the commands in line
    /// before this one (`ahead`) leave one free for it, so they keep their turn (decision 304).
    fn take_admitted(
        &self,
        states: &[State],
        ahead: &[Holder],
        warn: &mut dyn Write,
    ) -> Result<Option<Owned>> {
        let free = states.iter().filter(|s| s.is_free()).count();
        if ahead.len() < free {
            return self.take_free(states, warn);
        }
        Ok(None)
    }

    /// `--no-wait`: a free slot that the waiting commands in line leave over, or a refusal naming the
    /// holder, writing nothing.
    fn take_now(&self, warn: &mut dyn Write) -> Result<Owned> {
        let ahead = live_tickets(&self.dir.join(QUEUE), None, &*self.gate.probe, true)?;
        let states = self.states(&mut HashMap::new())?;
        if let Some(owned) = self.take_admitted(&states, &ahead, warn)? {
            return Ok(owned);
        }
        let why = blocker(&states, &ahead)
            .unwrap_or_else(|| "another command taking the last one".to_owned());
        bail!(
            "no free {} ({} = {} in config.toml): {why}; run this command again once it ends, or \
             without --no-wait to wait for a slot",
            self.pool.noun(),
            self.pool.setting(),
            self.count
        )
    }

    /// Waits in line for a free slot, saying who holds it once per holder.
    fn wait(&self, warn: &mut dyn Write) -> Result<Owned> {
        let queue = self.dir.join(QUEUE);
        let name = ticket_name();
        let ticket = Owned::create(&queue.join(&name), self.me, &self.gate.registry)
            .with_context(|| format!("cannot write a ticket in {}", queue.display()))?;
        let _waiting = Waiting::start();
        let mut unreadable = HashMap::new();
        let mut shown: Option<String> = None;
        loop {
            let ahead = live_tickets(&queue, Some(&name), &*self.gate.probe, true)?;
            let states = self.states(&mut unreadable)?;
            if let Some(owned) = self.take_admitted(&states, &ahead, warn)? {
                drop(ticket);
                return Ok(owned);
            }
            if let Some(why) = blocker(&states, &ahead)
                && shown.as_ref() != Some(&why)
            {
                writeln!(warn, "waiting for a {}: {why}", self.pool.noun())?;
                shown = Some(why);
            }
            // Dropping the ticket gives up this command's place in line.
            if self.gate.stop_asked() {
                return Err(Stopped.into());
            }
            if self.gate.catch_ctrl_c {
                catch_ctrl_c();
            }
            (self.gate.sleep)(POLL);
        }
    }
}

/// Counts this thread as waiting while it lives.
struct Waiting;

impl Waiting {
    fn start() -> Self {
        WAITING.fetch_add(1, Ordering::SeqCst);
        Waiting
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        WAITING.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Ctrl-C (installed once, by the first real wait): removes this process's slot and ticket files,
/// then exits. In `task loop`, whose own handler is installed first, that handler does it.
fn catch_ctrl_c() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let _ = ctrlc::set_handler(|| {
            release_all();
            if WAITING.load(Ordering::SeqCst) > 0 {
                eprintln!("stopped while waiting for a slot; nothing was created");
            }
            std::process::exit(130);
        });
    });
}

/// The holder a slot file left unreadable is named by: none is known.
fn dead_unreadable(path: &Path) -> Holder {
    Holder {
        process: Process {
            pid: 0,
            started_at: String::new(),
        },
        token: String::new(),
        command: format!("an unreadable {}", path.display()),
        task: None,
        since: "unknown".to_owned(),
    }
}

/// What keeps a command from a slot: the first live holder, else the first command in line.
fn blocker(states: &[State], ahead: &[Holder]) -> Option<String> {
    states
        .iter()
        .find_map(|state| match state {
            State::Held(holder) => Some(format!("held by {}", holder.describe())),
            _ => None,
        })
        .or_else(|| {
            ahead
                .first()
                .map(|holder| format!("{} is waiting ahead of it", holder.describe()))
        })
}

/// A ticket's name: the time it was made in nanoseconds, zero-padded so names sort by arrival,
/// then the pid and a counter, so two tickets made at once still differ.
fn ticket_name() -> String {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!(
        "{nanos:032}-{:010}-{:06}.json",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// The live tickets in `queue` older than `mine` (all of them for `None`), oldest first. A ticket
/// whose process is gone is skipped, and removed with `prune`; one not readable yet is still being
/// written, by a command that came after this look.
fn live_tickets(
    queue: &Path,
    mine: Option<&str>,
    probe: &dyn ProcessProbe,
    prune: bool,
) -> Result<Vec<Holder>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(queue).with_context(|| format!("cannot read {}", queue.display()))? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if name.ends_with(".json") && mine.is_none_or(|mine| name.as_str() < mine) {
            names.push(name);
        }
    }
    names.sort();
    let mut live = Vec::new();
    for name in names {
        let path = queue.join(&name);
        let Ok(text) = lock::read_file(&path) else {
            continue;
        };
        let Ok(holder) = serde_json::from_str::<Holder>(&text) else {
            continue;
        };
        if holder.process.alive(probe) {
            live.push(holder);
        } else if prune {
            remove_if_mine(&path, &text);
        }
    }
    Ok(live)
}

/// Who holds the `count` sandbox slots of `gate`'s folder now, in one line for `sbxm status`.
/// Reads only.
pub fn status_line(count: u32, gate: &Gate) -> String {
    status_line_in(Pool::Local, count, gate)
}

/// [`status_line`] for the `count` cloud slots (issue 599).
pub fn cloud_status_line(count: u32, gate: &Gate) -> String {
    status_line_in(Pool::Cloud, count, gate)
}

fn status_line_in(pool: Pool, count: u32, gate: &Gate) -> String {
    let mut line = format!("{}s: ", pool.noun());
    let Ok(dir) = gate.dir(pool) else {
        line.push_str(&format!("{count}, in a folder that can't be found"));
        return line;
    };
    let slots = Slots {
        pool,
        dir: &dir,
        count,
        gate,
        me: "",
    };
    let holders: Vec<String> = (1..=count)
        .filter_map(|k| match slots.state(k) {
            Ok(State::Held(holder)) => Some(holder.describe()),
            _ => None,
        })
        .collect();
    line.push_str(&format!("{} of {count} held", holders.len()));
    if !holders.is_empty() {
        line.push_str(&format!(" by {}", holders.join("; ")));
    }
    let waiting = live_tickets(&dir.join(QUEUE), None, &*gate.probe, false).map_or(0, |t| t.len());
    if waiting > 0 {
        line.push_str(&format!(", {waiting} waiting"));
    }
    line
}
