//! A scripted token endpoint and the saved-login files of issue 330 (decision 270).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use sbxm::auth::{self, Host, TokenEndpoint};

pub const NOW: u64 = 1_800_000_000;

/// One POST: the URL and the form's fields.
pub type Call = (String, Vec<(String, String)>);

/// Answers every POST with `status` and `body`, recording each form it was sent.
pub struct Endpoint {
    status: u16,
    body: String,
    pub calls: Mutex<Vec<Call>>,
}

impl Endpoint {
    pub fn answering(status: u16, body: &str) -> Self {
        Endpoint {
            status,
            body: body.to_owned(),
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn fresh(token: &str, expires_in: u64) -> Self {
        Self::answering(
            200,
            &format!(
                r#"{{"access_token":"{token}","expires_in":{expires_in},"token_type":"Bearer"}}"#
            ),
        )
    }

    pub fn calls(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    /// A [`Host`] answering with this endpoint, sbxm at `/opt/sbxm/sbxm`, the clock at [`NOW`].
    pub fn host(&self) -> Host<'_> {
        Host {
            endpoint: self,
            helper: helper_path(),
            now: NOW,
        }
    }
}

/// The absolute path a test's [`Host`] gives sbxm.
pub fn helper_path() -> PathBuf {
    std::env::temp_dir().join("sbxm-test-bin").join("sbxm")
}

impl TokenEndpoint for Endpoint {
    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> anyhow::Result<(u16, String)> {
        self.calls.lock().unwrap().push((
            url.to_owned(),
            form.iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        ));
        Ok((self.status, self.body.clone()))
    }
}

pub fn write_login(config_dir: &Path) {
    std::fs::create_dir_all(config_dir.join("auth")).unwrap();
    std::fs::write(
        auth::login_path(config_dir),
        r#"{"client_id":"fake-id.apps.googleusercontent.com","client_secret":"fake-secret","refresh_token":"fake-refresh"}"#,
    )
    .unwrap();
}

pub fn write_cache(config_dir: &Path, token: &str, expires_at: u64) {
    std::fs::write(
        auth::cache_path(config_dir),
        format!(r#"{{"access_token":"{token}","expires_at":{expires_at}}}"#),
    )
    .unwrap();
}
