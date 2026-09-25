use clap::Parser;

use sbxm::backend::SbxBackend;
use sbxm::cli::{Cli, Command, ConfigCommand};
use sbxm::commands;
use sbxm::config;
use sbxm::confirm::Terminal;

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::New {
            project,
            seed,
            profile,
        } => commands::new::run(
            &config::config_dir()?,
            &project,
            &commands::new::Options { seed, profile },
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
        Command::Open { project } => {
            commands::open::run(&config::config_dir()?, &project, &SbxBackend)
        }
        Command::Stop { project } => {
            commands::stop::run(&config::config_dir()?, &project, &SbxBackend)
        }
        Command::Rm {
            project,
            purge,
            yes,
        } => commands::rm::run(
            &config::config_dir()?,
            &project,
            &commands::rm::Options { purge, yes },
            &SbxBackend,
            &Terminal,
        ),
        Command::Config {
            command: ConfigCommand::Init,
        } => commands::config_init::run(),
    }
}
