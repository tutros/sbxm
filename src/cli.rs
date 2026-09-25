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
