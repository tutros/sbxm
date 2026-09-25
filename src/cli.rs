use std::path::PathBuf;

use clap::{Parser, Subcommand};

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
    },
    /// Stop a project's sandbox.
    Stop {
        /// Project name.
        project: String,
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
}
