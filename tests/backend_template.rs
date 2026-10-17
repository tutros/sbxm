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
