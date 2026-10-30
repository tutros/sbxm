//! The user's Google login for Antigravity sandboxes (decision 270, issue 330, spike S16): the
//! saved login (`<config dir>/auth/antigravity.json`, written by `sbxm auth antigravity`), the
//! cached access token (`<config dir>/auth/antigravity-access.json`) and its refresh. The tokens
//! stay on the host: a sandbox holds only placeholders, and `sbx`'s proxy swaps in the access
//! token `sbxm auth token antigravity` prints. No error here ever carries a token, the client
//! secret or a response body.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

/// Where Google refreshes an access token (`grant_type=refresh_token`).
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// The fix every login problem names.
pub const LOGIN_FIX: &str = "run `sbxm auth antigravity` to sign in again";

/// A cached token is refreshed when it expires within this many seconds (decision 270, point 2).
const REFRESH_MARGIN_S: u64 = 5 * 60;

/// `<config dir>/auth/antigravity.json`: the saved login.
pub fn login_path(config_dir: &Path) -> PathBuf {
    config_dir.join("auth").join("antigravity.json")
}

/// `<config dir>/auth/antigravity-access.json`: the cached access token.
pub fn cache_path(config_dir: &Path) -> PathBuf {
    config_dir.join("auth").join("antigravity-access.json")
}

/// One HTTP form POST, answered with its status and body. The real one runs on the host; tests
/// script it.
pub trait TokenEndpoint {
    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<(u16, String)>;
}

/// The host's way to reach [`TOKEN_URL`]. sbxm has no HTTP client of its own yet: which one to
/// use is left to the user (issue 330), so every refresh fails with that said.
pub struct HostEndpoint;

impl TokenEndpoint for HostEndpoint {
    fn post_form(&self, url: &str, _form: &[(&str, &str)]) -> Result<(u16, String)> {
        bail!(
            "this build of sbxm has no HTTP client to call {url} with (issue 330 leaves the \
             choice to the user)"
        )
    }
}

/// Seconds since the Unix epoch, for the token's expiry.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The saved login: `agy`'s own installed-app client and the user's refresh token. Its `Debug`
/// shows only the client id.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Login {
    pub client_id: String,
    pub client_secret: String,
    pub refresh_token: String,
}

impl std::fmt::Debug for Login {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Login")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("refresh_token", &"<redacted>")
            .finish()
    }
}

/// The cached access token and when it expires (Unix seconds).
#[derive(Serialize, Deserialize)]
struct Cached {
    access_token: String,
    expires_at: u64,
}

/// Reads the saved login; a missing file or field names the file and [`LOGIN_FIX`].
pub fn load_login(config_dir: &Path) -> Result<Login> {
    let path = login_path(config_dir);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => bail!(
            "no Antigravity login at {}; run `sbxm auth antigravity` to sign in",
            path.display()
        ),
        Err(e) => bail!("cannot read {}: {e}; {LOGIN_FIX}", path.display()),
    };
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|_| anyhow!("{} isn't JSON; {LOGIN_FIX}", path.display()))?;
    let field = |name: &str| -> Result<String> {
        match value.get(name).and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => Ok(s.to_owned()),
            _ => bail!("{} has no {name}; {LOGIN_FIX}", path.display()),
        }
    };
    Ok(Login {
        client_id: field("client_id")?,
        client_secret: field("client_secret")?,
        refresh_token: field("refresh_token")?,
    })
}

/// The access token `sbx`'s proxy swaps in: the cached one while it expires more than 5 minutes
/// after `now`, else a fresh one from [`refresh`]. A missing or unreadable cache is refreshed,
/// never an error.
pub fn access_token(config_dir: &Path, now: u64, endpoint: &dyn TokenEndpoint) -> Result<String> {
    if let Some(cached) = read_cache(config_dir)
        && cached.expires_at > now.saturating_add(REFRESH_MARGIN_S)
    {
        return Ok(cached.access_token);
    }
    refresh(config_dir, now, endpoint)
}

fn read_cache(config_dir: &Path) -> Option<Cached> {
    let text = std::fs::read_to_string(cache_path(config_dir)).ok()?;
    let cached: Cached = serde_json::from_str(&text).ok()?;
    (!cached.access_token.is_empty()).then_some(cached)
}

/// Asks Google for a new access token with the saved login and caches it. Anything but a 200
/// carrying a token is an error naming [`LOGIN_FIX`], never the body.
pub fn refresh(config_dir: &Path, now: u64, endpoint: &dyn TokenEndpoint) -> Result<String> {
    let login = load_login(config_dir)?;
    let (status, body) = endpoint
        .post_form(
            TOKEN_URL,
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", &login.refresh_token),
                ("client_id", &login.client_id),
                ("client_secret", &login.client_secret),
            ],
        )
        .map_err(|e| anyhow!("cannot reach {TOKEN_URL} to refresh the Antigravity login: {e:#}"))?;
    if status != 200 {
        bail!("Google refused to refresh the Antigravity login (HTTP {status}); {LOGIN_FIX}");
    }
    #[derive(Deserialize)]
    struct Answer {
        access_token: String,
        expires_in: u64,
    }
    let answer: Answer = serde_json::from_str(&body)
        .ok()
        .filter(|a: &Answer| !a.access_token.is_empty())
        .ok_or_else(|| {
            anyhow!("Google's answer to the Antigravity refresh had no access token; {LOGIN_FIX}")
        })?;
    let cached = Cached {
        access_token: answer.access_token,
        expires_at: now.saturating_add(answer.expires_in),
    };
    // Best effort: a cache that can't be written only costs the next call a refresh.
    let _ = write_private(
        &cache_path(config_dir),
        serde_json::to_string(&cached)?.as_bytes(),
    );
    Ok(cached.access_token)
}

/// Writes `bytes` to `path` through a temporary file in the same folder, readable by the user
/// only on Unix.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("a file path has a folder")?;
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = options
        .open(&tmp)
        .and_then(|mut f| f.write_all(bytes).and_then(|()| f.sync_all()))
        .and_then(|()| std::fs::rename(&tmp, path));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        bail!("cannot write {}: {e}", path.display());
    }
    Ok(())
}

/// The login is kept under the config dir, which must not be inside the base dir, where
/// workspaces live and get mounted into sandboxes.
pub fn check_config_dir(config_dir: &Path, base_dir: &Path) -> Result<()> {
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if canonical(config_dir).starts_with(canonical(base_dir)) {
        bail!(
            "the config dir {} is inside the base dir {}, where sandbox workspaces live, so the \
             Antigravity login can't be kept there; set SBXM_CONFIG_DIR to a folder outside it",
            config_dir.display(),
            base_dir.display()
        );
    }
    Ok(())
}
