//! `sbxm auth token antigravity` (decision 270, point 2): the helper `sbx`'s proxy runs for every
//! request it swaps the placeholder in, about once a second. It prints only the access token,
//! from the cache while it lasts, and nothing at all on a failure.

use std::io::Write;
use std::path::Path;

use anyhow::Result;

use crate::auth::{self, TokenEndpoint};

pub fn run(
    config_dir: &Path,
    now: u64,
    endpoint: &dyn TokenEndpoint,
    out: &mut dyn Write,
) -> Result<()> {
    let token = auth::access_token(config_dir, now, endpoint)?;
    out.write_all(token.as_bytes())?;
    out.flush()?;
    Ok(())
}
