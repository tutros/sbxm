//! The user's Google login for Antigravity sandboxes (decision 270, issue 330, spike S16): the
//! saved login (`<config dir>/auth/antigravity.json`, written by `sbxm auth antigravity`), the
//! cached access token (`<config dir>/auth/antigravity-access.json`) and its refresh. The tokens
//! stay on the host: a sandbox holds only placeholders, and `sbx`'s proxy swaps in the access
//! token `sbxm auth token antigravity` prints. No error here ever carries a token, the client
//! secret or a response body.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use crate::backend::{CustomSecretSpec, ExecSpec, SandboxBackend, SecretSource, Stdin};
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
    /// Whether this host can make the call at all, asked before any sandbox exists.
    fn check(&self) -> Result<()> {
        Ok(())
    }

    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<(u16, String)>;
}

/// The host's way to reach [`TOKEN_URL`] (decision 271): `ureq` over the platform's TLS and
/// its root certificates, through the proxy the environment names (`HTTPS_PROXY`, `NO_PROXY`).
pub struct HostEndpoint {
    /// How long connecting may take.
    pub connect: std::time::Duration,
    /// How long the whole call may take, the answer read included.
    pub overall: std::time::Duration,
}

impl HostEndpoint {
    /// The endpoint sbxm uses: 10 s to connect, 30 s in all.
    pub const GOOGLE: HostEndpoint = HostEndpoint {
        connect: std::time::Duration::from_secs(10),
        overall: std::time::Duration::from_secs(30),
    };
}

impl TokenEndpoint for HostEndpoint {
    /// Any answer is a status and a body: only a call that got none (no connection, a timeout)
    /// is an error, and it never carries the form.
    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<(u16, String)> {
        use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(self.connect))
            .timeout_global(Some(self.overall))
            .http_status_as_error(false)
            .tls_config(
                TlsConfig::builder()
                    .provider(TlsProvider::NativeTls)
                    .root_certs(RootCerts::PlatformVerifier)
                    .build(),
            )
            .build()
            .into();
        let mut response = agent
            .post(url)
            .send_form(form.iter().copied())
            .map_err(|e| anyhow!("{e}"))?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|e| anyhow!("cannot read the answer: {e}"))?;
        Ok((status, body))
    }
}

/// What a login-mode Antigravity sandbox needs from the host: the token endpoint for the
/// preflight's refresh, sbxm's own absolute path for the helper `sbx` runs, and the clock.
pub struct Host<'a> {
    pub endpoint: &'a (dyn TokenEndpoint + Sync),
    /// sbxm's own executable; `sbx` runs `<helper> auth token antigravity`.
    pub helper: PathBuf,
    /// Unix seconds.
    pub now: u64,
}

impl Host<'static> {
    /// The running sbxm and the real clock.
    pub fn real() -> Self {
        Host {
            endpoint: &HostEndpoint::GOOGLE,
            helper: std::env::current_exe().unwrap_or_default(),
            now: now(),
        }
    }
}

/// The login-mode checks made before any sandbox exists (decision 270, point 3): the config dir
/// is outside the base dir, sbxm knows its own absolute path, the login file is there and one
/// refresh succeeds. Each failure is one line naming the fix. Nothing is written, the cache
/// included: a later check may still refuse the command.
pub fn preflight(config_dir: &Path, base_dir: &Path, host: &Host) -> Result<()> {
    check_config_dir(config_dir, base_dir)?;
    if !host.helper.is_absolute() {
        bail!(
            "sbxm can't find its own absolute path ({}) to hand to sbx as the Antigravity token \
             helper; run sbxm by its full path, or set antigravity_auth = \"api-key\"",
            host.helper.display()
        );
    }
    helper_command(&host.helper)?;
    refresh_unsaved(config_dir, host.now, host.endpoint).map(|_| ())
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
    let cached = refresh_unsaved(config_dir, now, endpoint)?;
    save_cache(config_dir, &cached)?;
    Ok(cached.access_token)
}

/// `sbxm doctor`'s check (decision 270, point 5): the config dir is outside the base dir, the
/// saved login is there and one refresh succeeds. Nothing is written, the cache included.
pub fn check_login(
    config_dir: &Path,
    base_dir: &Path,
    now: u64,
    endpoint: &dyn TokenEndpoint,
) -> Result<()> {
    check_config_dir(config_dir, base_dir)?;
    refresh_unsaved(config_dir, now, endpoint).map(|_| ())
}

fn refresh_unsaved(config_dir: &Path, now: u64, endpoint: &dyn TokenEndpoint) -> Result<Cached> {
    let login = load_login(config_dir)?;
    match try_refresh(&login, now, endpoint)? {
        Ok(cached) => Ok(cached),
        Err(status) => {
            bail!("Google refused to refresh the Antigravity login (HTTP {status}); {LOGIN_FIX}")
        }
    }
}

/// One refresh with `login`: the new token, or the status of an answer that wasn't a 200 carrying
/// one (a 200 without a token counts as 200). Only an unreachable endpoint is an error.
fn try_refresh(
    login: &Login,
    now: u64,
    endpoint: &dyn TokenEndpoint,
) -> Result<std::result::Result<Cached, u16>> {
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
        .map_err(|e| {
            anyhow!(
                "cannot reach {TOKEN_URL} to refresh the Antigravity login: {e:#}; check this \
                 host's network, then {LOGIN_FIX} if it still fails"
            )
        })?;
    #[derive(Deserialize)]
    struct Answer {
        access_token: String,
        expires_in: u64,
    }
    if status != 200 {
        return Ok(Err(status));
    }
    Ok(serde_json::from_str(&body)
        .ok()
        .filter(|a: &Answer| !a.access_token.is_empty())
        .map(|answer| Cached {
            access_token: answer.access_token,
            expires_at: now.saturating_add(answer.expires_in),
        })
        .ok_or(status))
}

/// Caches `cached`. A failure is an error: without the cache `sbx`'s calls, about one a second,
/// would each ask Google for a new token (decision 270, point 2).
fn save_cache(config_dir: &Path, cached: &Cached) -> Result<()> {
    let path = cache_path(config_dir);
    let json = serde_json::to_string(cached)?;
    write_private(&path, json.as_bytes()).map_err(|e| {
        anyhow!(
            "cannot save the Antigravity token cache: {e:#}; make sure sbxm can replace that file"
        )
    })
}

/// Saves `login` once one of `pairs` (`agy`'s client id and secret, tried in order) refreshes it,
/// with that pair, and caches the access token it got (`sbxm auth antigravity`). Nothing is
/// written when no pair answers 200 with a token, and the login isn't saved when the cache can't
/// be: the helper `sbx` runs would fail on it too.
pub fn save_login(
    config_dir: &Path,
    refresh_token: &str,
    pairs: &[(String, String)],
    now: u64,
    endpoint: &dyn TokenEndpoint,
) -> Result<()> {
    let mut statuses = Vec::new();
    for (client_id, client_secret) in pairs {
        let login = Login {
            client_id: client_id.clone(),
            client_secret: client_secret.clone(),
            refresh_token: refresh_token.to_owned(),
        };
        match try_refresh(&login, now, endpoint)? {
            Ok(cached) => {
                save_cache(config_dir, &cached)?;
                write_private(
                    &login_path(config_dir),
                    serde_json::to_string(&login)?.as_bytes(),
                )?;
                return Ok(());
            }
            Err(status) => statuses.push(status.to_string()),
        }
    }
    bail!(
        "none of the {} client pairs found in agy refreshed the new login (HTTP {}): agy may have \
         changed how it signs in; set antigravity_auth = \"api-key\" meanwhile",
        pairs.len(),
        statuses.join(", ")
    )
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

/// The hosts `agy` sends the bearer token to (spike S16, Q1): the model calls and the user-info
/// lookup. The proxy swaps the placeholder only in requests to these.
pub const SECRET_HOSTS: [&str; 2] = ["daily-cloudcode-pa.googleapis.com", "www.googleapis.com"];

/// The environment variable the scoped secret's placeholder is set in inside the sandbox; `agy`
/// reads the token file, not this.
const SECRET_ENV: &str = "SBXM_AGY_TOKEN";

/// Where `agy` keeps its OAuth token, relative to the sandbox user's home.
const TOKEN_FILE: &str = ".gemini/antigravity-cli/antigravity-oauth-token";

/// The access-token placeholder of `sandbox`: in its token file and its scoped secret.
pub fn placeholder(sandbox: &str) -> String {
    format!("sbxm-agy-{sandbox}")
}

/// The scoped secret of a login-mode Antigravity sandbox, set before it is created: `sbx` runs
/// `<helper> auth token antigravity` on the host for the access token.
pub fn set_sandbox_secret(
    backend: &dyn SandboxBackend,
    sandbox: &str,
    helper: &Path,
) -> Result<()> {
    backend
        .set_custom_secret(&CustomSecretSpec {
            sandbox: sandbox.to_owned(),
            hosts: SECRET_HOSTS.map(str::to_owned).into(),
            env: SECRET_ENV.to_owned(),
            placeholder: placeholder(sandbox),
            source: SecretSource::CommandLine(helper_command(helper)?),
        })
        .with_context(|| format!("cannot store the Antigravity login secret for {sandbox}"))
}

/// The command line `sbx` runs for the access token: `<helper> auth token antigravity`, with the
/// path quoted the way of this host ([`helper_command_line`]).
pub fn helper_command(helper: &Path) -> Result<String> {
    let path = helper.to_str().with_context(|| {
        format!(
            "sbxm's path {} isn't valid Unicode, so it can't be put in the Antigravity token \
             helper's command line; set antigravity_auth = \"api-key\"",
            helper.display()
        )
    })?;
    helper_command_line(path, cfg!(windows))
}

/// `<path> auth token antigravity`, with `path` quoted so `sbx` runs that exact file. On Windows
/// `sbx` runs the text through `cmd` (checked by hand against `sbx` 0.46.0, issue 330): a
/// double-quoted path holding a space, `&`, `'`, `$`, `^`, `(` or `)` runs that file, but `%NAME%`
/// is expanded even inside the quotes, so a path with `%` (or `"`, which Windows paths can't
/// hold) is refused. On Unix the path is single-quoted, each `'` written as `'\''`, for a shell;
/// that form hasn't been tried against a real `sbx`.
pub fn helper_command_line(path: &str, windows: bool) -> Result<String> {
    let quoted = if windows {
        if path.contains(['%', '"']) {
            bail!(
                "sbxm's path {path} has a % or a \", which sbx's command line on Windows can't \
                 hold, so it can't be the Antigravity token helper; run sbxm from a folder \
                 without them, or set antigravity_auth = \"api-key\""
            );
        }
        format!("\"{path}\"")
    } else {
        format!("'{}'", path.replace('\'', r"'\''"))
    };
    Ok(format!("{quoted} auth token antigravity"))
}

/// Removes the scoped secret of `sandbox`, after the sandbox itself. The error names the command
/// that removes it by hand.
pub fn remove_sandbox_secret(backend: &dyn SandboxBackend, sandbox: &str) -> Result<()> {
    let placeholder = placeholder(sandbox);
    backend
        .remove_custom_secret(&placeholder, sandbox)
        .map_err(|e| {
            anyhow!(
                "cannot remove the Antigravity login secret of {sandbox}: {e:#}; run `sbx secret \
                 rm --placeholder {placeholder} --sandbox {sandbox} -f`"
            )
        })
}

/// Writes `agy`'s token file into the new sandbox `sandbox`: placeholders only, with a far-future
/// expiry so `agy` never refreshes it itself (spike S16, Q2). The file goes through stdin.
pub fn write_token_file(backend: &dyn SandboxBackend, sandbox: &str) -> Result<()> {
    let file = serde_json::json!({
        "access_token": placeholder(sandbox),
        "token_type": "Bearer",
        "refresh_token": "sbxm-refresh-token-stays-on-the-host",
        "expiry": "2099-01-01T00:00:00Z",
        "auth_method": "consumer",
        "id_token_email": "sbxm@placeholder.invalid",
    });
    let script = format!(
        "umask 077 && mkdir -p \"$HOME/$(dirname {TOKEN_FILE})\" && cat > \"$HOME/{TOKEN_FILE}\""
    );
    let output = backend
        .exec(
            sandbox,
            &ExecSpec {
                workdir: None,
                argv: vec!["sh".into(), "-c".into(), script],
                stdin: Stdin::Piped(file.to_string()),
            },
        )
        .with_context(|| format!("cannot write ~/{TOKEN_FILE} in {sandbox}"))?;
    if output.exit_code != Some(0) {
        bail!(
            "cannot write ~/{TOKEN_FILE} in {sandbox}: {}",
            output.stderr.trim()
        );
    }
    Ok(())
}
