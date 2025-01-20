//! This module defines the command-line interface (CLI) structure for the
//! application, using the clap crate for parsing and handling command-line
//! arguments.


use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Root command for the dotdeploy application
#[derive(Parser)]
#[command(
    author,
    version,
    about = "Dotdeploy - System configuration and dotfile manager"
)]
pub(crate) struct Cli {
    /// The subcommand to execute
    #[command(subcommand)]
    pub(crate) command: Commands,

    /// Show what would happen without making changes
    #[clap(long, short, action, global = true, env = "DOD_DRY_RUN")]
    pub(crate) dry_run: bool,

    /// Skip confirmations for destructive operations
    #[clap(long, short, action, global = true, env = "DOD_FORCE")]
    pub(crate) force: bool,

    /// Root folder of dotfiles
    #[clap(long, action, global = true, env = "DOD_CONFIG_ROOT")]
    pub(crate) config_root: Option<PathBuf>,

    /// Root folder of Dotedeploy modules
    #[clap(long, action, global = true, env = "DOD_MODULES_ROOT")]
    pub(crate) modules_root: Option<PathBuf>,

    /// Root folder of Dotedeploy hosts
    #[clap(long, action, global = true, env = "DOD_HOSTS_ROOT")]
    pub(crate) hosts_root:  Option<PathBuf>,

    /// Host device's hostname
    #[clap(long, action, global = true, env = "DOD_HOSTNAME")]
    pub(crate) hostname: Option<String>,

    /// Host device's Linux distribution
    #[clap(long, action, global = true, env = "DOD_DISTRIBUTION")]
    pub(crate) distribution: Option<String>,

    /// Use sudo to elevate privileges
    #[clap(long, action, global = true, env = "DOD_USE_SUDO")]
    pub(crate) use_sudo: bool,

    /// Deploy files to directories other than the user's HOME
    #[clap(long, action, global = true, env = "DOD_DEPLOY_SYS_FILES")]
    pub(crate) deploy_sys_files: bool,

    /// Command used to install packages
    #[clap(long, action, global = true, env = "DOD_INSTALL_PKG_CMD", num_args = 0.., value_delimiter = ' ')]
    pub(crate) install_pkg_cmd: Option<Vec<String>>,

    /// Command used to remove packages
    #[clap(long, action, global = true, env = "DOD_REMOVE_PKG_CMD", num_args = 0.., value_delimiter = ' ')]
    pub(crate) remove_pkg_cmd: Option<Vec<String>>,

    /// Skip package installation during deployment
    #[clap(long, action, global = true, env = "DOD_SKIP_PKG_INSTALL")]
    pub(crate) skip_pkg_install: bool,

    /// Assume "yes" instead of prompting
    #[clap(short = 'y', long = "noconfirm", global = true, env = "DOD_YES")]
    pub(crate) noconfirm: bool,

    /// Verbosity level (-v = debug, -vv = trace)
    #[clap(
        short,
        long,
        action = clap::ArgAction::Count,
        global = true
    )]
    pub(crate) verbosity: u8,
}

/// Available subcommands for dotdeploy
#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Deploy system configuration or specific modules
    Deploy {
        /// Optional list of module names to deploy
        modules: Option<Vec<String>>,
    },

    /// Remove deployed modules and restore backups
    Remove {
        /// Optional list of module names to remove
        modules: Option<Vec<String>>,
    },

    /// Update module content and optionally installed packages
    Update {
        /// Also update installed packages
        #[clap(long, short)]
        packages: bool,
    },

    /// Synchronize deployed files with their sources
    Sync {
        /// Automatically sync without asking
        #[clap(long)]
        auto: bool,
    },

    /// Validate deployment state and check for differences
    Validate {
        /// Show detailed differences between source and deployed files
        #[clap(long)]
        diff: bool,

        /// Enter interactive fix mode for discrepancies
        #[clap(long)]
        fix: bool,
    },

    /// Complete uninstall of dotdeploy configuration
    Nuke {
        /// Skip safety confirmations
        #[clap(long)]
        really: bool,
    },
}

/// Parses command-line arguments and returns a configured Cli instance.
///
/// This function handles argument parsing and applies any necessary
/// post-processing, such as capping the verbosity level at 2 (-vv maximum).
///
/// # Returns
///
/// Returns the parsed and processed CLI configuration.
pub(crate) fn get_cli() -> Cli {
    let mut cli = Cli::parse();

    // Cap verbosity at level 2 (trace)
    cli.verbosity = cli.verbosity.min(2);

    cli
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_cli() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
