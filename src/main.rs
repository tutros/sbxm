use clap::Parser;

use sbxm::backend::SbxBackend;
use sbxm::cli::{Cli, Command, ConfigCommand};
use sbxm::commands;
use sbxm::config;

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::New { project, seed } => commands::new::run(
            &config::config_dir()?,
            &project,
            &commands::new::Options { seed },
            &SbxBackend,
        ),
        Command::List { json } => {
            let entries = commands::list::entries(&config::config_dir()?, &SbxBackend)?;
            let render = if json {
                commands::list::render_json
            } else {
                commands::list::render_table
            };
            print!("{}", render(&entries));
            Ok(())
        }
        Command::Stop { project } => {
            commands::stop::run(&config::config_dir()?, &project, &SbxBackend)
        }
        Command::Config {
            command: ConfigCommand::Init,
        } => commands::config_init::run(),
    }
}
