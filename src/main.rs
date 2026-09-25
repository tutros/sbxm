mod cli;
mod commands;
mod config;

use clap::Parser;

use cli::{Cli, Command, ConfigCommand};

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Config {
            command: ConfigCommand::Init,
        } => commands::config_init::run(),
    }
}
