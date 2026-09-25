mod cli;
mod commands;
mod config;
mod project;

use clap::Parser;

use cli::{Cli, Command, ConfigCommand};

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::New { project } => commands::new::run(&project),
        Command::Config {
            command: ConfigCommand::Init,
        } => commands::config_init::run(),
    }
}
