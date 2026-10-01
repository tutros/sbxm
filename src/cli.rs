use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::harness::Harness;

/// Per-project Docker Sandboxes from a shared, versioned config.
#[derive(Debug, Parser)]
#[command(name = "sbxm", bin_name = "sbxm", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create a project and its sandbox.
    New {
        /// Project name: lowercase letters, digits and '-'.
        project: String,
        /// Copy this directory's contents into the new project.
        #[arg(long)]
        seed: Option<PathBuf>,
        /// Profile to apply (default: `default_profile` in the global config).
        #[arg(long)]
        profile: Option<String>,
        /// Agent harness to run in the sandbox.
        #[arg(long, value_enum, default_value_t)]
        harness: Harness,
    },
    /// List sbxm sandboxes and flag orphans.
    List {
        /// Print JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Attach to a project's sandbox, creating it if needed.
    Open {
        /// Project name.
        project: String,
        /// Recreate the sandbox from the current config; its session history is lost,
        /// the workspace is kept.
        #[arg(long)]
        rebuild: bool,
        /// Which of the project's sandboxes to open.
        #[arg(long, value_enum)]
        harness: Harness,
    },
    /// Stop a project's sandbox.
    Stop {
        /// Project name.
        project: String,
        /// Which of the project's sandboxes to stop.
        #[arg(long, value_enum)]
        harness: Harness,
    },
    /// Remove a project's sandbox and state; the workspace is kept unless --purge.
    Rm {
        /// Project name.
        project: String,
        /// Also delete the workspace and sbxm's metadata, after confirmation.
        #[arg(long)]
        purge: bool,
        /// Don't ask for confirmation (needed for --purge without a terminal).
        #[arg(long, requires = "purge")]
        yes: bool,
        /// Which of the project's sandboxes to remove (--purge removes all).
        #[arg(
            long,
            value_enum,
            conflicts_with = "purge",
            required_unless_present = "purge"
        )]
        harness: Option<Harness>,
    },
    /// Check sbx, the config and the base dir; exits non-zero if any check fails.
    Doctor,
    /// Run comparisons between coding agents.
    Run(RunArgs),
    /// Carry GitHub issues and PRs through worker, gates, review and hand-off.
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
    /// Manage sbxm configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Write a starter global config and `default` profile.
    Init,
    /// Print the merged config and its hash, without creating anything.
    Show {
        /// Merge this project's sandbox.toml and use its recorded profile.
        project: Option<String>,
        /// Profile to show (default: the project's recorded one, then `default_profile`).
        #[arg(long)]
        profile: Option<String>,
        /// Also print the kits `sbxm new` would generate.
        #[arg(long)]
        kits: bool,
        /// Which harness's config and kits to show.
        #[arg(long, value_enum, default_value_t)]
        harness: Harness,
    },
    /// Print the folder profiles are read from (for scripts), or fail like any command would.
    ProfilesDir,
}

/// `sbxm run <config>` launches a comparison; `sbxm run init` writes a starter
/// config. (A config file named `init` is written `./init`.)
#[derive(Debug, clap::Args)]
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
pub struct RunArgs {
    #[command(subcommand)]
    pub command: Option<RunCommand>,
    /// The run-config file (create one with `sbxm run init`).
    #[arg(required = true)]
    pub config: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum RunCommand {
    /// Write a starter run-config (default: ./run.toml); refuses to overwrite.
    Init {
        /// Where to write it.
        path: Option<PathBuf>,
    },
    /// Print a saved run: status, answer and diff summary per contestant.
    Show {
        /// The run id `sbxm run` printed, e.g. 2026-09-30-a1b2c3.
        run_id: String,
        /// Also print every full patch.
        #[arg(long)]
        diff: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum TaskCommand {
    /// Write a starter sbxm-task.toml (default: in the working directory); refuses to overwrite.
    Init {
        /// The repo's root folder.
        path: Option<PathBuf>,
    },
    /// Start a task for each chosen GitHub issue: a worker agent in its own sandbox, then its
    /// commits are collected. Blocks until the workers finish.
    Start {
        /// Start this issue (repeatable).
        #[arg(long = "issue", value_name = "N", conflicts_with = "workers")]
        issues: Vec<u32>,
        /// Start up to N issues, chosen by label (must-fix first), skipping questions, blocked
        /// and related ones.
        #[arg(long, value_name = "N", conflicts_with = "issues")]
        workers: Option<usize>,
        /// The worker's harness (default: sbxm-task.toml, else claude).
        #[arg(long, value_enum)]
        worker_harness: Option<Harness>,
        /// The worker's model (default: the harness's own).
        #[arg(long)]
        worker_model: Option<String>,
        /// The worker's time limit, e.g. 90s, 45m, 2h.
        #[arg(long)]
        time_limit: Option<String>,
        /// The sandbox profile (default: sbxm-task.toml).
        #[arg(long)]
        profile: Option<String>,
        /// The branch to start from (default: the repo's default branch).
        #[arg(long)]
        base: Option<String>,
        /// The GitHub repo, owner/name (default: this checkout's origin).
        #[arg(long)]
        repo: Option<String>,
    },
    /// Show the tasks: stage, status, and whether one was interrupted.
    Status {
        /// Only the task for this issue.
        #[arg(long, conflicts_with = "pr")]
        issue: Option<u32>,
        /// Only the task for this PR.
        #[arg(long)]
        pr: Option<u32>,
        /// Print the task records as JSON.
        #[arg(long)]
        json: bool,
    },
    /// File the findings of a review as GitHub issues, one per finding. A dry run: nothing is
    /// filed or changed.
    FileFindings {
        /// A review file, e.g. sdlc/reviews/2026-10-01-milestone-2b.md.
        #[arg(long, value_name = "F", required = true)]
        file: PathBuf,
        /// File only these finding ids (repeatable, or comma-separated).
        #[arg(long = "only", value_name = "ID", value_delimiter = ',')]
        only: Vec<String>,
        /// File findings that have no acceptance criteria with only the standard ones.
        #[arg(long)]
        standard_criteria: bool,
        /// Keep personal paths instead of replacing them with ~.
        #[arg(long)]
        keep_paths: bool,
        /// The GitHub repo, owner/name (default: this checkout's origin).
        #[arg(long)]
        repo: Option<String>,
    },
}
