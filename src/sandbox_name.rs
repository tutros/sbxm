//! The 63-character cap on every sandbox name sbxm makes (issue 556): a name is
//! `<stem><id><suffix>` (`sbxm-task-` + a task id + `-review-codex`, say), and one longer than a
//! hostname's 63 characters keeps its stem and suffix whole and shortens only the id, to its
//! leading part, the id's own hash or number, and a hash of the whole id, so two ids don't share a
//! capped name. A name within the limit is unchanged, so the sandboxes of existing tasks keep their
//! names.

use sha2::{Digest, Sha256};

/// The longest sandbox name sbxm makes: a hostname label's limit.
pub const MAX_LEN: usize = 63;

/// How many hex digits of the id's SHA-256 a capped name keeps.
const HASH_LEN: usize = 6;

/// `<stem><id><suffix>`, or, when that is longer than [`MAX_LEN`], the id shortened so the name
/// fits: its leading part, then a marker that keeps it apart from other ids: the id's own hash or
/// number ([`own_tail`]) and `-<hash>` ([`hash`]), or, for an id without one, 12 hex digits of
/// its SHA-256.
/// Ids, stems and suffixes are ASCII (`[a-z0-9-]`), so a cut never splits a character, and the
/// capped name has no character the full one lacks (no period: the cloud rejects those).
pub fn capped(stem: &str, id: &str, suffix: &str) -> String {
    let full = format!("{stem}{id}{suffix}");
    if full.len() <= MAX_LEN {
        return full;
    }
    let (lead, marker) = match own_tail(id) {
        Some(tail) => (
            &id[..id.len() - tail.len() - 1],
            format!("{tail}-{}", hash(id)),
        ),
        None => (id, long_hash(id)),
    };
    let room = MAX_LEN.saturating_sub(stem.len() + suffix.len() + 1 + marker.len());
    let mut end = room.min(lead.len());
    while !lead.is_char_boundary(end) {
        end -= 1;
    }
    let lead = lead[..end].trim_end_matches('-');
    if lead.is_empty() {
        format!("{stem}{marker}{suffix}")
    } else {
        format!("{stem}{lead}-{marker}{suffix}")
    }
}

/// The first 6 hex digits of `id`'s SHA-256: in every capped name, so a name tells ids apart.
pub fn hash(id: &str) -> String {
    long_hash(id)[..HASH_LEN].to_owned()
}

/// The first 12 hex digits of `id`'s SHA-256: the marker of an id with no hash of its own.
fn long_hash(id: &str) -> String {
    let digest = Sha256::digest(id.as_bytes());
    digest[..6].iter().map(|b| format!("{b:02x}")).collect()
}

/// The id's own hash or number (review M-1): its last `-` part when that is 1 to 12 hex digits
/// and something comes before it (a spec id's path hash, a fix id's commit, a run id's random
/// part, an issue's number). A capped name keeps it whole and adds [`hash`] of the whole id, so
/// two ids share a capped name only if both their own hashes and their whole-id hashes match.
fn own_tail(id: &str) -> Option<&str> {
    let (lead, tail) = id.rsplit_once('-')?;
    let hex = |b: u8| b.is_ascii_digit() || matches!(b, b'a'..=b'f');
    (!lead.is_empty() && !tail.is_empty() && tail.len() <= 12 && tail.bytes().all(hex))
        .then_some(tail)
}

/// What follows the id part of `name`, when `name` is `<stem>` and then `id` itself, or exactly
/// the capped name [`capped`] makes of `id` and that suffix; `None` otherwise (so another id that
/// merely starts like a capped one never counts as `id`'s).
pub fn after_id<'a>(name: &'a str, stem: &str, id: &str) -> Option<&'a str> {
    let rest = name.strip_prefix(stem)?;
    if let Some(after) = rest.strip_prefix(id) {
        return Some(after);
    }
    rest.match_indices('-').find_map(|(at, _)| {
        let after = &rest[at..];
        (capped(stem, id, after) == name).then_some(after)
    })
}
