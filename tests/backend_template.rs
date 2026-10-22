//! Milestone 3 slice 3: `FakeBackend` records template calls and keeps a template list.

use sbxm::backend::{FakeBackend, SandboxBackend, TemplateImage};

fn image(repository: &str, tag: &str) -> TemplateImage {
    TemplateImage {
        id: "0123456789ab".into(),
        repository: repository.into(),
        tag: tag.into(),
        flavor: "claude-code-docker".into(),
        created_at: "2026-10-16T00:00:00Z".into(),
        size: 1,
    }
}

#[test]
fn a_saved_template_is_listed_and_logged() {
    let fake = FakeBackend::default();

    fake.template_save("sbxm-template-dev-claude", "sbxm-dev-claude:abc")
        .unwrap();

    let listed: Vec<_> = fake
        .template_ls()
        .unwrap()
        .iter()
        .map(TemplateImage::reference)
        .collect();
    assert_eq!(listed, ["sbxm-dev-claude:abc"]);
    assert_eq!(
        fake.log(),
        ["template-save sbxm-template-dev-claude sbxm-dev-claude:abc"]
    );
}

#[test]
fn a_removed_template_leaves_the_list_and_is_logged() {
    let fake = FakeBackend::default().with_templates(vec![
        image("docker.io/library/sbxm-dev-claude", "abc"),
        image("docker.io/library/sbxm-dev-claude", "def"),
    ]);

    fake.template_rm("sbxm-dev-claude:abc").unwrap();

    let listed: Vec<_> = fake
        .template_ls()
        .unwrap()
        .iter()
        .map(TemplateImage::reference)
        .collect();
    assert_eq!(listed, ["sbxm-dev-claude:def"]);
    assert_eq!(fake.log(), ["template-rm sbxm-dev-claude:abc"]);
}

// Slice 3 review S-1: the fake applies the same guard as the real backend.
#[test]
fn fake_refuses_to_remove_a_builtin_template() {
    let builtin = image("docker.io/docker/sandbox-templates", "claude-code-docker");
    let fake = FakeBackend::default().with_templates(vec![builtin.clone()]);

    let err = fake.template_rm(&builtin.reference()).unwrap_err();

    assert!(
        format!("{err:#}").contains("refusing to remove template"),
        "{err:#}"
    );
    assert_eq!(fake.template_ls().unwrap(), [builtin]);
    assert!(fake.log().is_empty(), "{:?}", fake.log());
}

#[test]
fn the_shared_guard_accepts_an_sbxm_tag_and_refuses_others() {
    assert!(
        sbxm::backend::check_template_tag("sbxm-dev-claude:2a9974ee6210-20261015t2210").is_ok()
    );
    assert!(sbxm::backend::check_template_tag("claude-code-docker").is_err());
}

// Slice 3 review S-3: saving onto an existing tag replaces it, as sbx does (S11, question 8).
#[test]
fn saving_onto_an_existing_tag_replaces_it() {
    let fake = FakeBackend::default();

    fake.template_save("sbxm-template-dev-claude", "sbxm-dev-claude:abc")
        .unwrap();
    fake.template_save("sbxm-template-dev-claude", "sbxm-dev-claude:abc")
        .unwrap();

    assert_eq!(fake.template_ls().unwrap().len(), 1);
}
