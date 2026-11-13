//! Issue 556: every sandbox name sbxm makes is at most 63 characters. A longer one keeps its stem
//! and role suffix and shortens the id to its leading part and a hash of the whole id; one within
//! the limit is unchanged.

use sbxm::sandbox_name::{MAX_LEN, after_id, capped, hash};
use sbxm::task::record;

/// A valid spec id whose `sbxm-task-<id>-review-codex` name is `len` characters long.
fn spec_id_for_review_len(len: usize) -> String {
    let fixed = "sbxm-task-".len() + "spec-".len() + "-0b24b8".len() + "-review-codex".len();
    format!("spec-{}-0b24b8", "a".repeat(len - fixed))
}

fn cloud_valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_LEN
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

#[test]
fn a_short_task_name_is_unchanged() {
    assert_eq!(
        record::sandbox_name("issue-602", "claude"),
        "sbxm-task-issue-602-claude"
    );
}

#[test]
fn a_name_of_exactly_63_characters_is_unchanged() {
    let id = spec_id_for_review_len(63);
    let name = record::sandbox_name(&id, "review-codex");
    assert_eq!(name, format!("sbxm-task-{id}-review-codex"));
    assert_eq!(name.len(), 63);
}

#[test]
fn names_of_64_and_65_characters_are_capped_keeping_the_role_and_a_hash() {
    for len in [64, 65] {
        let id = spec_id_for_review_len(len);
        let name = record::sandbox_name(&id, "review-codex");
        assert!(name.len() <= MAX_LEN, "{len}: {name}");
        assert!(name.starts_with("sbxm-task-spec-a"), "{name}");
        assert!(
            name.ends_with(&format!("-{}-review-codex", hash(&id))),
            "{name}"
        );
        assert!(cloud_valid(&name), "{name}");
    }
}

#[test]
fn the_2026_11_10_reviewer_name_is_capped() {
    let id = "spec-decision-316-medium-auto-merge-0b24b8";
    assert_eq!(format!("sbxm-task-{id}-review-codex").len(), 65);
    let name = record::sandbox_name(id, "review-codex");
    assert_eq!(name.len(), 63, "{name}");
    assert!(name.starts_with("sbxm-task-spec-decision-316-"), "{name}");
    assert!(
        name.ends_with(&format!("-0b24b8-{}-review-codex", hash(id))),
        "{name}"
    );
    assert!(name.ends_with("-review-codex"), "{name}");
    // The worker's name was 59 characters, and stays as it was.
    assert_eq!(
        record::sandbox_name(id, "claude"),
        format!("sbxm-task-{id}-claude")
    );
}

#[test]
fn a_200_character_id_is_capped_unique_and_ends_with_the_role() {
    let a = format!("spec-{}-0b24b8", "x".repeat(200));
    let b = format!("spec-{}-0b24b9", "x".repeat(200));
    let (na, nb) = (
        record::sandbox_name(&a, "review-antigravity"),
        record::sandbox_name(&b, "review-antigravity"),
    );
    assert!(na.len() <= MAX_LEN && nb.len() <= MAX_LEN, "{na} {nb}");
    assert!(na.ends_with("-review-antigravity"), "{na}");
    assert_ne!(na, nb);
    assert!(cloud_valid(&na) && cloud_valid(&nb), "{na} {nb}");
}

#[test]
fn two_long_ids_sharing_their_first_50_characters_get_different_names() {
    let shared = "spec-".to_owned() + &"b".repeat(45);
    assert_eq!(shared.len(), 50);
    let a = format!("{shared}-one-0b24b8");
    let b = format!("{shared}-two-0b24b8");
    let (na, nb) = (
        record::sandbox_name(&a, "claude"),
        record::sandbox_name(&b, "claude"),
    );
    assert!(na.len() <= MAX_LEN && nb.len() <= MAX_LEN, "{na} {nb}");
    assert_ne!(na, nb);
}

#[test]
fn a_cut_that_ends_on_a_hyphen_leaves_no_double_hyphen() {
    // `room` lands right after a `-` of the id.
    let name = capped("s-", "ab-cdefghijklmnop", &"r".repeat(MAX_LEN - 2 - 3 - 13));
    assert!(name.starts_with("s-ab-"), "{name}");
    assert!(!name.contains("--"), "{name}");
    assert!(name.len() <= MAX_LEN, "{name}");
}

#[test]
fn a_capped_name_is_recognized_as_its_tasks_and_no_other() {
    let id = "spec-decision-316-medium-auto-merge-0b24b8";
    let name = record::sandbox_name(id, "review-codex");
    assert_eq!(after_id(&name, "sbxm-task-", id), Some("-review-codex"));
    assert_eq!(
        record::sandbox_role(&name, id).as_deref(),
        Some("review-codex")
    );
    let other = "spec-decision-316-medium-auto-merge-0b24b9";
    assert_eq!(record::sandbox_role(&name, other), None);
    // An uncapped name is still recognized as before.
    assert_eq!(
        record::sandbox_role("sbxm-task-issue-602-claude", "issue-602").as_deref(),
        Some("claude")
    );
    assert_eq!(
        record::sandbox_role("sbxm-task-issue-602-claude", "issue-60"),
        None
    );
}

#[test]
fn run_and_template_names_are_capped_too() {
    use sbxm::harness::Harness;
    let run = sbxm::run::sandbox_name("2026-09-29-a1b2c3", "0", 1);
    assert_eq!(run, "sbxm-run-2026-09-29-a1b2c3-0-1");
    let long = sbxm::run::sandbox_name("2026-09-29-a1b2c3", "judge", u64::MAX);
    assert!(long.len() <= MAX_LEN, "{long}");
    assert!(sbxm::run::names_sandbox_of(&long, "2026-09-29-a1b2c3"));
    let id = "r".repeat(60);
    let capped_run = sbxm::run::sandbox_name(&id, "jev", 3);
    assert!(capped_run.len() <= MAX_LEN, "{capped_run}");
    assert!(capped_run.ends_with("-jev-3"), "{capped_run}");
    assert!(sbxm::run::names_sandbox_of(&capped_run, &id));
    assert!(!sbxm::run::names_sandbox_of(&capped_run, &"r".repeat(59)));

    let profile = "p".repeat(40);
    let template = sbxm::template::sandbox_name(&profile, Harness::Antigravity);
    assert!(template.len() <= MAX_LEN, "{template}");
    assert!(template.ends_with("-antigravity"), "{template}");
    assert_eq!(
        sbxm::template::sandbox_name("dev", Harness::Claude),
        "sbxm-template-dev-claude"
    );
}

#[test]
fn the_longest_project_sandbox_name_fits_without_a_cap() {
    let name = sbxm::project::sandbox_name(&"p".repeat(40), "antigravity");
    assert!(name.len() <= MAX_LEN, "{name}");
}

#[test]
fn sbxm_status_names_the_task_of_a_capped_sandbox() {
    use sbxm::status::sandboxes::{self, Owner};
    let base = tempfile::tempdir().unwrap();
    let id = "spec-decision-316-medium-auto-merge-0b24b8";
    for task in [id, "spec-decision-316-medium-auto-merge-0b24b9"] {
        let dir = base.path().join(".sbxm").join("tasks").join(task);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("task.json"), "{}").unwrap();
    }
    let claims = sandboxes::claims(base.path());
    let name = record::sandbox_name(id, "review-codex");
    assert_eq!(sandboxes::owner(&claims, &name), Owner::Task(id.into()));
}

#[test]
fn only_the_exact_capped_form_counts_as_a_tasks_capped_name() {
    let id = "spec-foo-0b24b8";
    // Another id that starts with what a capped name of `id` would: its name isn't `id`'s.
    let other = format!("spec-foo-{}-bar-123456", hash(id));
    let name = record::sandbox_name(&other, "claude");
    assert_eq!(record::sandbox_role(&name, id), None, "{name}");
    assert_eq!(after_id(&name, "sbxm-task-", id), None, "{name}");
}

// ---- review M-1: a capped name keeps the id's own hash or number --------------------------

/// Two valid spec ids (of real paths) whose whole-id hashes share their first 6 hex digits.
const SAME_HASH6: [&str; 2] = [
    "spec-ssssssssssssssssssssssssssssssssssssssss-0ecd72",
    "spec-ssssssssssssssssssssssssssssssssssssssss-8dc4e8",
];

#[test]
fn ids_whose_hashes_share_6_digits_keep_their_own_hash_and_differ() {
    let [a, b] = SAME_HASH6;
    assert_eq!(hash(a), hash(b));
    let (na, nb) = (
        record::sandbox_name(a, "claude"),
        record::sandbox_name(b, "claude"),
    );
    assert_ne!(na, nb);
    assert!(na.len() <= MAX_LEN && nb.len() <= MAX_LEN, "{na} {nb}");
    assert!(na.contains("-0ecd72-") && na.ends_with("-claude"), "{na}");
    assert!(nb.contains("-8dc4e8-") && nb.ends_with("-claude"), "{nb}");
    assert_eq!(record::sandbox_role(&na, b), None);
    assert_eq!(record::sandbox_role(&nb, a), None);
    assert_eq!(record::sandbox_role(&na, a).as_deref(), Some("claude"));
}

#[test]
fn an_id_without_its_own_hash_gets_a_longer_one() {
    // A template's profile name ends in no hash: the capped name carries 12 hex digits.
    let profile = "p".repeat(60);
    let name = capped("sbxm-template-", &profile, "-claude");
    assert!(name.len() <= MAX_LEN, "{name}");
    let marker = name
        .strip_suffix("-claude")
        .and_then(|n| n.rsplit_once('-'))
        .map(|(_, m)| m.to_owned())
        .unwrap();
    assert_eq!(marker.len(), 12, "{name}");
    assert!(marker.starts_with(&hash(&profile)), "{name}");
    assert_eq!(after_id(&name, "sbxm-template-", &profile), Some("-claude"));
}
