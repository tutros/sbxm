use clap::Parser;

use sbxm::backend::SbxBackend;
use sbxm::cli::{Cli, Command, ConfigCommand, RunArgs, RunCommand, TaskCommand};
use sbxm::commands;
use sbxm::config;
use sbxm::confirm::Terminal;
use sbxm::task;

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
        Command::Run(RunArgs {
            command: Some(RunCommand::Init { path }),
            ..
        }) => {
            let written = commands::run_init::run(path.as_deref())?;
            println!("Wrote {}", written.display());
            println!(
                "Edit the task and contestants, then run: sbxm run {}",
                written.display()
            );
            Ok(())
        }
        Command::Run(RunArgs {
            command: None,
            config: Some(config_path),
        }) => {
            commands::run::run(
                &config::config_dir()?,
                &config_path,
                &SbxBackend,
                &mut std::io::stdout(),
                &mut std::io::stderr(),
            )?;
            Ok(())
        }
        Command::Run(RunArgs {
            command: Some(RunCommand::Show { run_id, diff }),
            ..
        }) => {
            let text = commands::run_show::render(
                &config::config_dir()?,
                &run_id,
                &commands::run_show::Options { full_diff: diff },
            )?;
            print!("{text}");
            Ok(())
        }
        Command::Task {
            command: TaskCommand::Init { path },
        } => {
            let written = commands::task_init::run(path.as_deref())?;
            println!("Wrote {}", written.display());
            println!("Check the profile and gates, then run: sbxm task gates --dry-run");
            Ok(())
        }
        Command::Task {
            command: TaskCommand::Status { issue, pr, json },
        } => {
            let which = match (issue, pr) {
                (Some(n), _) => Some((task::record::Kind::Issue, n)),
                (None, Some(n)) => Some((task::record::Kind::Pr, n)),
                (None, None) => None,
            };
            let global = config::GlobalConfig::load(&config::config_dir()?)?;
            print!(
                "{}",
                commands::task_status::render(
                    &global.base_dir,
                    which,
                    json,
                    &task::record::SystemProbe,
                )?
            );
            Ok(())
        }
        Command::Run(_) => unreachable!("clap requires a config or a subcommand"),
        Command::Config {
            command: ConfigCommand::Init,
        } => commands::config_init::run(),
        Command::Config {
            command: ConfigCommand::ProfilesDir,
        } => {
            print!(
                "{}",
                commands::config_profiles_dir::render(&config::config_dir()?)?
            );
            Ok(())
        }
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
