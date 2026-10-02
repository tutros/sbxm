//! PR 57 review, M-1: sbxm's own files go into `<workspace>/.sbxm-task` without following a
//! link the agent (or a branch it committed) put there.

mod common;

use std::fs;

use common::dir_link;
use sbxm::task::repo::{Existing, write_agent_files};

const FILES: [(&str, &[u8]); 2] = [("review.md", b"the review"), ("fix-prompt.md", b"fix it")];

#[test]
fn files_are_written_into_a_new_agent_folder() {
    let tmp = tempfile::tempdir().unwrap();

    write_agent_files(tmp.path(), &FILES, Existing::Refuse).unwrap();

    let dir = tmp.path().join(".sbxm-task");
    assert_eq!(fs::read(dir.join("review.md")).unwrap(), b"the review");
    assert_eq!(fs::read(dir.join("fix-prompt.md")).unwrap(), b"fix it");
}

#[test]
fn a_real_folder_with_other_files_keeps_them_and_old_copies_are_replaced() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join(".sbxm-task");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("issue.md"), "keep").unwrap();
    fs::write(dir.join("review.md"), "old").unwrap();

    write_agent_files(tmp.path(), &FILES, Existing::Refuse).unwrap();

    assert_eq!(fs::read(dir.join("issue.md")).unwrap(), b"keep");
    assert_eq!(fs::read(dir.join("review.md")).unwrap(), b"the review");
}

#[test]
fn a_linked_agent_folder_is_refused_and_its_target_is_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tmp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("review.md"), "precious").unwrap();
    let workspace = tmp.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    dir_link(&workspace.join(".sbxm-task"), &outside);

    let err = write_agent_files(&workspace, &FILES, Existing::Refuse).unwrap_err();

    assert!(format!("{err:#}").contains(".sbxm-task"), "{err:#}");
    assert_eq!(fs::read(outside.join("review.md")).unwrap(), b"precious");
    assert!(!outside.join("fix-prompt.md").exists());
}

#[test]
fn a_linked_agent_folder_in_a_fresh_checkout_is_replaced_not_followed() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tmp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("review.md"), "precious").unwrap();
    let workspace = tmp.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    dir_link(&workspace.join(".sbxm-task"), &outside);

    write_agent_files(&workspace, &FILES, Existing::Replace).unwrap();

    assert_eq!(fs::read(outside.join("review.md")).unwrap(), b"precious");
    assert!(!outside.join("fix-prompt.md").exists());
    let dir = workspace.join(".sbxm-task");
    assert!(!fs::symlink_metadata(&dir).unwrap().file_type().is_symlink());
    assert_eq!(fs::read(dir.join("review.md")).unwrap(), b"the review");
}

#[test]
fn a_tracked_file_in_place_of_the_folder_is_replaced_in_a_fresh_checkout_and_refused_otherwise() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join(".sbxm-task"), "not a folder").unwrap();

    assert!(write_agent_files(tmp.path(), &FILES, Existing::Refuse).is_err());
    write_agent_files(tmp.path(), &FILES, Existing::Replace).unwrap();

    assert_eq!(
        fs::read(tmp.path().join(".sbxm-task").join("review.md")).unwrap(),
        b"the review"
    );
}

#[cfg(unix)]
#[test]
fn a_linked_destination_file_is_refused_or_replaced_and_its_target_is_untouched() {
    use common::file_link;
    for existing in [Existing::Refuse, Existing::Replace] {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("precious.txt");
        fs::write(&target, "precious").unwrap();
        let dir = tmp.path().join(".sbxm-task");
        fs::create_dir(&dir).unwrap();
        file_link(&dir.join("review.md"), &target);

        let result = write_agent_files(tmp.path(), &FILES, existing);

        assert_eq!(fs::read(&target).unwrap(), b"precious");
        match existing {
            Existing::Refuse => assert!(result.is_err()),
            Existing::Replace => {
                result.unwrap();
                assert_eq!(fs::read(dir.join("review.md")).unwrap(), b"the review");
            }
        }
    }
}
