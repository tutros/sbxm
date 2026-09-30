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
            harness,
        } => commands::new::run(
            &config::config_dir()?,
            &project,
            &commands::new::Options {
                seed,
                profile,
                harness,
                ..Default::default()
            },
            &SbxBackend,
            &mut std::io::stderr(),
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
        Command::Open {
            project,
            rebuild,
            harness,
        } => commands::open::run(
            &config::config_dir()?,
            &project,
            &commands::open::Options { rebuild, harness },
            &SbxBackend,
            &mut std::io::stderr(),
        ),
        Command::Stop { project, harness } => {
            commands::stop::run(&config::config_dir()?, &project, harness, &SbxBackend)
        }
        Command::Rm {
            project,
            purge,
            yes,
            harness,
        } => commands::rm::run(
            &config::config_dir()?,
            &project,
            &commands::rm::Options {
                purge,
                yes,
                harness: harness.unwrap_or_default(),
            },
            &SbxBackend,
            &Terminal,
        ),
        Command::Doctor => {
            let report = commands::doctor::run(
                &config::config_dir()?,
                &SbxBackend,
                &commands::doctor::Host::real(),
            );
            print!("{}", report.render());
            if report.failed() {
                std::process::exit(1);
            }
            Ok(())
        }
        Command::Config {
            command: ConfigCommand::Init,
        } => commands::config_init::run(),
        Command::Config {
            command:
                ConfigCommand::Show {
                    project,
                    profile,
                    kits,
                    harness,
                },
        } => {
            let options = commands::config_show::Options {
                profile,
                kits,
                harness,
            };
            let output = commands::config_show::render(
                &config::config_dir()?,
                project.as_deref(),
                &options,
            )?;
            print!("{output}");
            Ok(())
        }
    }
}
