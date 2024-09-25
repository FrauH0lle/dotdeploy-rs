//! This module defines the command-line interface (CLI) structure for the application, using the
//! clap crate for parsing and handling command-line arguments.

use clap::{Parser, Subcommand};

// Represents the main command-line interface structure.
// This struct defines the overall CLI, including global options and subcommands.
/// dotdeploy -- System configuraton and dotfile manager
#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub(crate) struct Cli {
    /// The subcommand to be executed (Deploy or Remove).
    #[command(subcommand)]
    pub(crate) command: Commands,

    /// Verbosity level for output.
    ///
    /// Can be specified up to 2 times (-v or -vv) to increase detail.
    #[clap(short = 'v', long = "verbose", action = clap::ArgAction::Count, global = true)]
    pub(crate) verbosity: u8,

    /// Flag to skip package installation during deployment.
    #[clap(long, short, action)]
    pub(crate) skip_pkg_install: bool,
}

/// Enumerates the available subcommands for the application.
#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Deploy system configuration or specific modules.
    Deploy {
        /// Optional list of module names to deploy.
        modules: Option<Vec<String>>,
    },

    /// Remove system configuration or specific modules.
    Remove {
        /// Optional list of module names to remove.
        modules: Option<Vec<String>>,
    },
}

/// Parses command-line arguments and returns a configured Cli instance.
///
/// This function handles the parsing of arguments and applies any necessary post-processing, such
/// as capping the verbosity level.
///
/// # Returns
///
/// A Cli struct representing the parsed command-line arguments.
pub(crate) fn get_cli() -> Cli {
    let mut cli = Cli::parse();

    // Cap the verbosity level at 2
    cli.verbosity = std::cmp::min(2, cli.verbosity);

    cli
}
