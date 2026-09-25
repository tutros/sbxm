use clap::Parser;

use sbxm::backend::SbxBackend;
use sbxm::cli::{Cli, Command, ConfigCommand};
use sbxm::commands;
use sbxm::config;

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::New { project } => {
            commands::new::run(&config::config_dir()?, &project, &SbxBackend)
        }
        Command::Config {
            command: ConfigCommand::Init,
        } => commands::config_init::run(),
    }
}
