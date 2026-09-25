use clap::Parser;

use sbxm::cli::{Cli, Command, ConfigCommand};
use sbxm::commands;

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::New { project } => commands::new::run(&project),
        Command::Config {
            command: ConfigCommand::Init,
        } => commands::config_init::run(),
    }
}
