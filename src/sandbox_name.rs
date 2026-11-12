//! The 63-character cap on every sandbox name sbxm makes (issue 556): a name is
//! `<stem><id><suffix>` (`sbxm-task-` + a task id + `-review-codex`, say), and one longer than a
//! hostname's 63 characters keeps its stem and suffix whole and shortens only the id, to its
//! leading part and a hash of the whole id, so two ids never share a capped name. A name within the
//! limit is unchanged, so the sandboxes of existing tasks keep their names.

use sha2::{Digest, Sha256};

/// The longest sandbox name sbxm makes: a hostname label's limit.
pub const MAX_LEN: usize = 63;

/// How many hex digits of the id's SHA-256 a capped name keeps.
const HASH_LEN: usize = 6;

/// `<stem><id><suffix>`, or, when that is longer than [`MAX_LEN`], the id shortened to its
/// leading part and `-<hash>` (the first 6 hex digits of the id's SHA-256) so the name fits.
/// Ids, stems and suffixes are ASCII (`[a-z0-9-]`), so a cut never splits a character, and the
/// capped name has no character the full one lacks (no period: the cloud rejects those).
pub fn capped(stem: &str, id: &str, suffix: &str) -> String {
    let full = format!("{stem}{id}{suffix}");
    if full.len() <= MAX_LEN {
        return full;
    }
    let room = MAX_LEN.saturating_sub(stem.len() + suffix.len() + 1 + HASH_LEN);
    format!("{stem}{}{suffix}", shortened(id, room))
}

/// The first 6 hex digits of `id`'s SHA-256: the part of a capped name that tells ids apart.
pub fn hash(id: &str) -> String {
    let digest = Sha256::digest(id.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    hex[..HASH_LEN].to_owned()
}

/// `id`'s first `keep` characters (trailing `-` dropped) and `-<hash>`; the hash alone when none
/// are kept.
fn shortened(id: &str, keep: usize) -> String {
    let mut end = keep.min(id.len());
    while !id.is_char_boundary(end) {
        end -= 1;
    }
    let lead = id[..end].trim_end_matches('-');
    if lead.is_empty() {
        hash(id)
    } else {
        format!("{lead}-{}", hash(id))
    }
}

/// What follows the id part of `name`, when `name` is `<stem>` and then `id` itself, or exactly
/// the capped name [`capped`] makes of `id` and that suffix; `None` otherwise (so another id that
/// merely starts like a capped one never counts as `id`'s).
pub fn after_id<'a>(name: &'a str, stem: &str, id: &str) -> Option<&'a str> {
    let rest = name.strip_prefix(stem)?;
    if let Some(after) = rest.strip_prefix(id) {
        return Some(after);
    }
    rest.match_indices(hash(id).as_str()).find_map(|(at, _)| {
        let after = &rest[at + HASH_LEN..];
        (capped(stem, id, after) == name).then_some(after)
    })
}
