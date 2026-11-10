//! The loop's notifications to the user (issue 530, decision 311): a Windows toast when the loop
//! needs the user, through the [`Notifier`] trait so the loop's rules of when are tested with
//! [`FakeNotifier`] and only `main` wires the real [`Toast`]. Each event has a key, and the keys
//! shown go into `<base>/.sbxm/notified.json` ([`Shown`]), so a restarted loop never shows one
//! twice.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use anyhow::{Context, Result, bail};

/// One event worth the user's attention.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    /// What makes it the same event again (answer 4): shown once, ever.
    pub key: String,
    /// The event and its target: `PR #526 awaits your merge (medium)`.
    pub title: String,
    /// One line of reason, then the next step sbxm names.
    pub body: String,
    /// What a click opens: the PR or issue URL, else the loop's log file as a `file:` URL.
    pub click: String,
}

/// Shows a [`Notification`]; `out` takes a line where no toast can be shown.
pub trait Notifier {
    fn show(&self, note: &Notification, out: &mut dyn Write) -> Result<()>;
}

/// Records what it was asked to show, for tests; with `fail`, every show fails after recording.
#[derive(Default)]
pub struct FakeNotifier {
    shown: Mutex<Vec<Notification>>,
    pub fail: bool,
}

impl FakeNotifier {
    pub fn failing() -> Self {
        Self {
            fail: true,
            ..Self::default()
        }
    }

    pub fn shown(&self) -> Vec<Notification> {
        self.shown.lock().unwrap().clone()
    }
}

impl Notifier for FakeNotifier {
    fn show(&self, note: &Notification, _out: &mut dyn Write) -> Result<()> {
        self.shown.lock().unwrap().push(note.clone());
        if self.fail {
            bail!("FakeNotifier: the toast could not be shown");
        }
        Ok(())
    }
}

/// The environment variable the toast script reads its XML from, so no text of the toast is ever
/// part of a command line.
pub const XML_VAR: &str = "SBXM_TOAST_XML";

/// The toast snippet of `scripts/factory-trigger.ps1` (issue 327): WinRT types load only in
/// Windows PowerShell 5.1, shown as Windows PowerShell's own app.
pub const TOAST_SCRIPT: &str = "\
[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null
[Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null
$doc = New-Object Windows.Data.Xml.Dom.XmlDocument
$doc.LoadXml($env:SBXM_TOAST_XML)
$app = '{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\WindowsPowerShell\\v1.0\\powershell.exe'
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($app).Show([Windows.UI.Notifications.ToastNotification]::new($doc))
";

/// The real notifier: a toast on Windows, one line on `out` elsewhere.
pub struct Toast<'a> {
    pub windows: bool,
    /// Runs Windows PowerShell 5.1 with a script, the toast's XML in [`XML_VAR`].
    pub powershell: &'a (dyn Fn(&str, &str) -> Result<()> + 'a),
}

impl Toast<'static> {
    /// This system's: `powershell.exe` on Windows.
    pub fn real() -> Self {
        Self {
            windows: cfg!(windows),
            powershell: &run_powershell,
        }
    }
}

impl Notifier for Toast<'_> {
    fn show(&self, note: &Notification, out: &mut dyn Write) -> Result<()> {
        if !self.windows {
            writeln!(
                out,
                "notification: {}: {} ({})",
                note.title, note.body, note.click
            )?;
            return Ok(());
        }
        (self.powershell)(TOAST_SCRIPT, &toast_xml(note))
    }
}

fn run_powershell(script: &str, xml: &str) -> Result<()> {
    let output = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .env(XML_VAR, xml)
        .output()
        .context("cannot run powershell.exe")?;
    if !output.status.success() {
        bail!(
            "powershell.exe failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// `text` safe inside an XML element or a quoted attribute, as PowerShell's
/// `[Security.SecurityElement]::Escape` makes it.
pub fn escape_xml(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            c => escaped.push(c),
        }
    }
    escaped
}

/// The toast's XML: title and body as text, a click opening `click` (protocol activation).
pub fn toast_xml(note: &Notification) -> String {
    format!(
        "<toast activationType=\"protocol\" launch=\"{}\"><visual><binding \
         template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>",
        escape_xml(&note.click),
        escape_xml(&note.title),
        escape_xml(&note.body)
    )
}

/// `path` as a `file:` URL, for a click that opens a file.
pub fn file_url(path: &Path) -> String {
    let text = path.to_string_lossy().replace(char::from(92), "/");
    if text.starts_with('/') {
        format!("file://{text}")
    } else {
        format!("file:///{text}")
    }
}

/// `<base>/.sbxm/notified.json`: the keys of the notifications shown, so none is shown twice.
pub fn shown_path(base_dir: &Path) -> PathBuf {
    base_dir.join(".sbxm").join("notified.json")
}

/// The keys shown so far, read once at the loop's start and written after each new one.
pub struct Shown {
    path: PathBuf,
    keys: BTreeSet<String>,
}

impl Shown {
    /// Reads [`shown_path`]: none yet when it is missing; an unreadable one is a warning on
    /// `warn` and read as empty (answer 4).
    pub fn load(base_dir: &Path, warn: &mut dyn Write) -> Result<Self> {
        let path = shown_path(base_dir);
        let read = match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str::<BTreeSet<String>>(&text).map_err(|e| e.into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeSet::new()),
            Err(e) => Err(anyhow::Error::from(e)),
        };
        let keys = match read {
            Ok(keys) => keys,
            Err(e) => {
                writeln!(
                    warn,
                    "warning: cannot read {} ({e:#}); read as empty, so a notification shown \
                     before may be shown again",
                    path.display()
                )?;
                BTreeSet::new()
            }
        };
        Ok(Self { path, keys })
    }

    pub fn contains(&self, key: &str) -> bool {
        self.keys.contains(key)
    }

    /// Adds `key` and writes the file, through a temporary file renamed over it.
    pub fn record(&mut self, key: &str) -> Result<()> {
        if !self.keys.insert(key.to_owned()) {
            return Ok(());
        }
        let text = serde_json::to_string_pretty(&self.keys)?;
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
        }
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, text).with_context(|| format!("cannot write {}", tmp.display()))?;
        if let Err(e) = fs::rename(&tmp, &self.path) {
            let _ = fs::remove_file(&tmp);
            return Err(e).with_context(|| format!("cannot write {}", self.path.display()));
        }
        Ok(())
    }
}
