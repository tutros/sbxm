//! What `sbxm template` asks of the tasks (issue 539): the template side never names
//! `crate::task`, so the commands hand it [`TemplateTasks`], which reads the task records and a
//! repo's `sbxm-task.toml`.

use std::path::Path;

use anyhow::Result;

use super::{config, record};
use crate::template::{self, User, Warm};

/// The tasks under a base folder, as the template side sees them.
pub struct TemplateTasks;

impl template::Tasks for TemplateTasks {
    fn users(&self, base_dir: &Path, tag: &str) -> Result<Vec<User>> {
        let mut users = Vec::new();
        for task in record::load_all(base_dir)? {
            let agents = [&task.worker, &task.reviewer];
            if agents
                .iter()
                .any(|a| a.as_ref().and_then(|a| a.template.as_deref()) == Some(tag))
            {
                users.push(User::Task {
                    id: task.id,
                    kind: task.kind,
                    number: task.number,
                });
            }
        }
        Ok(users)
    }

    fn warm(&self, repo: &Path) -> Result<Option<Warm>> {
        config::load_warm(repo)
    }
}
