use clap::Parser;

use sbxm::backend::SbxBackend;
use sbxm::cli::{Cli, Command, ConfigCommand, GateTier, RunArgs, RunCommand, TaskCommand};
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
        Command::Task {
            command:
                TaskCommand::Start {
                    issues,
                    workers,
                    worker_harness,
                    worker_model,
                    time_limit,
                    profile,
                    base,
                    repo,
                    restart,
                    yes,
                },
        } => commands::task_start::run_with(
            &config::config_dir()?,
            &commands::task_start::Options {
                repo_root: std::env::current_dir()?,
                issues,
                workers,
                worker_harness,
                worker_model,
                time_limit,
                profile,
                base,
                repo,
                clone_source: None,
                identity: None,
            },
            restart.then_some(&commands::task_start::Restart {
                confirm: &Terminal,
                yes,
            }),
            &SbxBackend,
            &sbxm::github::gh::GhBackend::default(),
            &task::record::SystemProbe,
            &mut std::io::stdout(),
            &mut std::io::stderr(),
        ),
        Command::Task {
            command:
                TaskCommand::Gates {
                    issue,
                    tier,
                    dry_run,
                },
        } => commands::task_gates::run(
            &config::config_dir()?,
            &commands::task_gates::Options {
                repo_root: std::env::current_dir()?,
                issue,
                tiers: match tier {
                    GateTier::Sandbox => task::pipeline::Tiers::SANDBOX,
                    GateTier::Host => task::pipeline::Tiers::HOST,
                    GateTier::All => task::pipeline::Tiers::ALL,
                },
                dry_run,
            },
            &SbxBackend,
            &task::record::SystemProbe,
            &task::gates::ShellHostRunner,
            &mut std::io::stdout(),
        ),
        Command::Task {
            command:
                TaskCommand::FileFindings {
                    issue,
                    pr,
                    file,
                    create,
                    only,
                    standard_criteria,
                    keep_paths,
                    repo,
                },
        } => {
            use commands::task_file_findings::Source;
            let source = match (issue, pr, file) {
                (Some(number), ..) => Source::Task {
                    base_dir: config::GlobalConfig::load(&config::config_dir()?)?.base_dir,
                    kind: task::record::Kind::Issue,
                    number,
                },
                (None, Some(number), _) => Source::Task {
                    base_dir: config::GlobalConfig::load(&config::config_dir()?)?.base_dir,
                    kind: task::record::Kind::Pr,
                    number,
                },
                (None, None, Some(file)) => Source::File(file),
                (None, None, None) => unreachable!("clap requires one of --issue, --pr, --file"),
            };
            commands::task_file_findings::run(
                &commands::task_file_findings::Options {
                    source,
                    repo_root: std::env::current_dir()?,
                    repo,
                    create,
                    standard_criteria,
                    keep_paths,
                    only,
                },
                &sbxm::github::gh::GhBackend::default(),
                &mut std::io::stdout(),
                &mut std::io::stderr(),
            )
        }
        Command::Task {
            command:
                TaskCommand::Review {
                    issue,
                    reviewer_harness,
                    reviewer_model,
                    reviewer_time_limit,
                    time_limit,
                    profile,
                },
        } => commands::task_review::run(
            &config::config_dir()?,
            &commands::task_review::Options {
                repo_root: std::env::current_dir()?,
                issue,
                reviewer_harness,
                reviewer_model,
                reviewer_time_limit,
                time_limit,
                profile,
            },
            &SbxBackend,
            &task::record::SystemProbe,
            &task::gates::ShellHostRunner,
            &mut std::io::stdout(),
            &mut std::io::stderr(),
        ),
        Command::Task {
            command:
                TaskCommand::Run {
                    issue,
                    worker_harness,
                    worker_model,
                    reviewer_harness,
                    reviewer_model,
                    time_limit,
                    reviewer_time_limit,
                    profile,
                    base,
                    repo,
                    restart,
                    yes,
                },
        } => commands::task_run::run(
            &config::config_dir()?,
            &commands::task_run::Options {
                repo_root: std::env::current_dir()?,
                issue,
                worker_harness,
                worker_model,
                reviewer_harness,
                reviewer_model,
                time_limit,
                reviewer_time_limit,
                profile,
                base,
                repo,
                clone_source: None,
                identity: None,
            },
            restart.then_some(&commands::task_start::Restart {
                confirm: &Terminal,
                yes,
            }),
            &SbxBackend,
            &sbxm::github::gh::GhBackend::default(),
            &task::record::SystemProbe,
            &task::gates::ShellHostRunner,
            &mut std::io::stdout(),
            &mut std::io::stderr(),
        ),
        Command::Task {
            command: TaskCommand::Finish { issue },
        } => commands::task_finish::run(
            &config::config_dir()?,
            &commands::task_finish::Options { issue },
            &sbxm::github::gh::GhBackend::default(),
            &task::record::SystemProbe,
            &mut std::io::stdout(),
        ),
        Command::Task {
            command: TaskCommand::Rm { issue, pr, yes },
        } => {
            let (kind, number) = match (issue, pr) {
                (Some(n), _) => (task::record::Kind::Issue, n),
                (None, Some(n)) => (task::record::Kind::Pr, n),
                (None, None) => unreachable!("clap requires --issue or --pr"),
            };
            commands::task_rm::run(
                &config::config_dir()?,
                &commands::task_rm::Options { kind, number, yes },
                &SbxBackend,
                &task::record::SystemProbe,
                &Terminal,
                &mut std::io::stdout(),
            )
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
