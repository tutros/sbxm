//! M2a slice 6: run IDs and the atomic reservation of a run's two roots
//! (P2): `<date>-<6 lowercase hex>`, created with create-dir-that-must-not-exist,
//! never touching existing files.

use sbxm::run::id;
use tempfile::TempDir;

fn is_hex(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

#[test]
fn generated_ids_are_a_date_and_six_hex_digits() {
    let generated = id::generate();

    assert_eq!(generated.len(), "2026-09-30-a1b2c3".len(), "{generated}");
    let (date, suffix) = generated.split_at(10);
    assert!(id::is_valid(&generated), "{generated}");
    assert_eq!(&date[4..5], "-");
    assert_eq!(&suffix[..1], "-");
    assert!(is_hex(&suffix[1..]), "{generated}");
}

#[test]
fn consecutive_ids_differ() {
    let ids: std::collections::HashSet<String> = (0..50).map(|_| id::generate()).collect();
    assert_eq!(ids.len(), 50);
}

#[test]
fn days_since_the_epoch_become_calendar_dates() {
    assert_eq!(id::date_from_days(0), "1970-01-01");
    assert_eq!(id::date_from_days(11016), "2000-02-29");
    assert_eq!(id::date_from_days(20726), "2026-09-30");
    assert_eq!(id::date_from_days(20727), "2026-10-01");
}

#[test]
fn validation_accepts_only_the_exact_format() {
    assert!(id::is_valid("2026-09-30-a1b2c3"));
    for bad in [
        "",
        "2026-09-30-A1B2C3",
        "2026-09-30-a1b2c",
        "2026-09-30-a1b2c34",
        "2026-9-30-a1b2c3",
        "2026-09-30-a1b2cg",
        "../2026-09-30-a1b2c3",
        "2026-09-30-a1b2c3/",
        "2026-09-30-a1b2c3\\x",
        "2026-09-30-a1b2c3 ",
        "latest",
    ] {
        assert!(!id::is_valid(bad), "{bad:?}");
    }
}

#[test]
fn reserving_creates_both_roots_and_returns_their_paths() {
    let base = TempDir::new().unwrap();

    let roots = id::reserve_with(base.path(), ["2026-09-30-aaaaaa".to_owned()]).unwrap();

    assert_eq!(roots.id, "2026-09-30-aaaaaa");
    assert_eq!(roots.meta, base.path().join(".sbxm/runs/2026-09-30-aaaaaa"));
    assert_eq!(roots.workspaces, base.path().join("runs/2026-09-30-aaaaaa"));
    assert!(roots.meta.is_dir() && roots.workspaces.is_dir());
}

#[test]
fn a_collision_moves_on_to_the_next_candidate_without_touching_the_old_files() {
    let base = TempDir::new().unwrap();
    let taken = base.path().join(".sbxm/runs/2026-09-30-aaaaaa");
    std::fs::create_dir_all(&taken).unwrap();
    std::fs::write(taken.join("precious.txt"), "keep").unwrap();

    let roots = id::reserve_with(
        base.path(),
        [
            "2026-09-30-aaaaaa".to_owned(),
            "2026-09-30-bbbbbb".to_owned(),
        ],
    )
    .unwrap();

    assert_eq!(roots.id, "2026-09-30-bbbbbb");
    assert_eq!(
        std::fs::read_to_string(taken.join("precious.txt")).unwrap(),
        "keep"
    );
    // The colliding candidate got no workspace root either.
    assert!(!base.path().join("runs/2026-09-30-aaaaaa").exists());
}

#[test]
fn a_taken_workspace_root_also_counts_and_leaves_no_half_reserved_meta_root() {
    let base = TempDir::new().unwrap();
    std::fs::create_dir_all(base.path().join("runs/2026-09-30-aaaaaa")).unwrap();

    let roots = id::reserve_with(
        base.path(),
        [
            "2026-09-30-aaaaaa".to_owned(),
            "2026-09-30-bbbbbb".to_owned(),
        ],
    )
    .unwrap();

    assert_eq!(roots.id, "2026-09-30-bbbbbb");
    assert!(!base.path().join(".sbxm/runs/2026-09-30-aaaaaa").exists());
}

#[test]
fn running_out_of_candidates_refuses_and_changes_nothing() {
    let base = TempDir::new().unwrap();
    let taken = base.path().join(".sbxm/runs/2026-09-30-aaaaaa");
    std::fs::create_dir_all(&taken).unwrap();

    let err = id::reserve_with(base.path(), ["2026-09-30-aaaaaa".to_owned()])
        .unwrap_err()
        .to_string();

    assert!(err.contains("could not reserve a run id"), "{err}");
    assert_eq!(std::fs::read_dir(&taken).unwrap().count(), 0);
    assert!(!base.path().join("runs").join("2026-09-30-aaaaaa").exists());
}

#[test]
fn reserve_picks_a_fresh_valid_id() {
    let base = TempDir::new().unwrap();

    let roots = id::reserve(base.path()).unwrap();

    assert!(id::is_valid(&roots.id));
    assert!(roots.meta.is_dir() && roots.workspaces.is_dir());
}
