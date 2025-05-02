//! This module defines the command-line interface (CLI) structure using clap's builder pattern

use clap::builder::{EnumValueParser, FalseyValueParser, TypedValueParser};
use clap::{
    Arg, ArgAction, ArgMatches, Command, ValueEnum, crate_name, crate_version, value_parser,
};
use clap_complete::Shell;
use std::env;
use std::{ffi::OsString, path::PathBuf};

/// Synchronization components that can be targeted by sync operations
///
/// Specifies which parts of a module should be processed during synchronization. Used to
/// selectively sync files, tasks, packages, or all components.
#[derive(ValueEnum, Clone, Debug)]
pub(crate) enum SyncComponent {
    #[value(name = "files")]
    Files,
    #[value(name = "tasks")]
    Tasks,
    #[value(name = "packages")]
    Packages,
    #[value(name = "all")]
    All,
}

impl std::str::FromStr for SyncComponent {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "files" => Ok(SyncComponent::Files),
            "tasks" => Ok(SyncComponent::Tasks),
            "packages" => Ok(SyncComponent::Packages),
            "all" => Ok(SyncComponent::All),
            _ => Err(format!("Unknown sync component: {}", s)),
        }
    }
}

impl SyncComponent {
    /// Returns `true` if the sync component is [`All`].
    ///
    /// [`All`]: SyncComponent::All
    #[must_use]
    pub(crate) fn is_all(&self) -> bool {
        matches!(self, Self::All)
    }

    /// Returns `true` if the sync component is [`Packages`].
    ///
    /// [`Packages`]: SyncComponent::Packages
    #[must_use]
    pub(crate) fn is_packages(&self) -> bool {
        matches!(self, Self::Packages)
    }

    /// Returns `true` if the sync component is [`Tasks`].
    ///
    /// [`Tasks`]: SyncComponent::Tasks
    #[must_use]
    pub(crate) fn is_tasks(&self) -> bool {
        matches!(self, Self::Tasks)
    }

    /// Returns `true` if the sync component is [`Files`].
    ///
    /// [`Files`]: SyncComponent::Files
    #[must_use]
    pub(crate) fn is_files(&self) -> bool {
        matches!(self, Self::Files)
    }
}

// -------------------------------------------------------------------------------------------------
// CLI builder
// -------------------------------------------------------------------------------------------------

/// Constructs the CLI application definition using clap's builder pattern
///
/// Defines all commands, arguments, and help documentation.
pub(crate) fn build_cli() -> Command {
    let cmd = Command::new(crate_name!())
        .version(crate_version!())
        .about("Dotdeploy - System configuration and dotfile manager")
        .subcommand_required(true)
        // --
        // * Main and global options
        .arg(
            Arg::new("dry_run")
                .long("dry-run")
                .short('d')
                .global(true)
                .env("DOD_DRY_RUN")
                .value_parser(FalseyValueParser::new().map(|b| -> u8 {
                    if b { 1 } else { 0 }
                }))
                .action(ArgAction::Count)
                .help("Show what would happen without making changes - NOT IMPLEMENTED"),
        )
        .arg(
            Arg::new("ask")
                .long("ask")
                .global(true)
                .value_parser(FalseyValueParser::new().map(|b| -> u8 {
                    if b { 1 } else { 0 }
                }))
                .action(ArgAction::Count)
                .help("Prompt for confirmation [default]"),
        )
        .arg(
            Arg::new("no_ask")
                .short('y')
                .long("no-ask")
                .alias("yes")
                .global(true)
                .env("DOD_YES")
                .value_parser(FalseyValueParser::new().map(|b| -> u8 {
                    if b { 1 } else { 0 }
                }))
                .action(ArgAction::Count)
                .help("Assume \"yes\" instead of prompting"),
        )
        .arg(
            Arg::new("force")
                .short('f')
                .long("force")
                .global(true)
                .env("DOD_FORCE")
                .value_parser(FalseyValueParser::new().map(|b| -> u8 {
                    if b { 1 } else { 0 }
                }))
                .action(ArgAction::Count)
                .help("Skip confirmations for destructive operations"),
        )
        .arg(
            Arg::new("no_force")
                .long("no-force")
                .global(true)
                .value_parser(FalseyValueParser::new().map(|b| -> u8 {
                    if b { 1 } else { 0 }
                }))
                .action(ArgAction::Count)
                .help("Don't skip confirmations for destructive operations [default]"),
        )
        .arg(
            Arg::new("config_file")
                .long("config-file")
                .env("DOD_CONFIG_FILE")
                .value_parser(value_parser!(PathBuf)),
        )
        .arg(
            Arg::new("dotfiles_root")
                .long("dotfiles-root")
                .env("DOD_DOTFILES_ROOT")
                .value_parser(value_parser!(PathBuf)),
        )
        .arg(
            Arg::new("modules_root")
                .long("modules-root")
                .env("DOD_MODULES_ROOT")
                .value_parser(value_parser!(PathBuf)),
        )
        .arg(
            Arg::new("hosts_root")
                .long("hosts-root")
                .env("DOD_HOSTS_ROOT")
                .value_parser(value_parser!(PathBuf)),
        )
        .arg(
            Arg::new("hostname")
                .long("hostname")
                .env("DOD_HOSTNAME")
                .value_parser(value_parser!(String)),
        )
        .arg(
            Arg::new("distribution")
                .long("distribution")
                .env("DOD_DISTRIBUTION")
                .value_parser(value_parser!(String)),
        )
        .arg(
            Arg::new("use_sudo")
                .long("use-sudo")
                .env("DOD_USE_SUDO")
                .value_parser(FalseyValueParser::new().map(|b| -> u8 {
                    if b { 1 } else { 0 }
                }))
                .action(ArgAction::Count)
                .help("Allow privilege elevation [default]"),
        )
        .arg(
            Arg::new("no_use_sudo")
                .long("no-use-sudo")
                .value_parser(FalseyValueParser::new().map(|b| -> u8 {
                    if b { 1 } else { 0 }
                }))
                .action(ArgAction::Count)
                .help("Don't allow privilege elevation"),
        )
        .arg(
            Arg::new("sudo_cmd")
                .long("sudo-cmd")
                .env("DOD_SUDO_CMD")
                .value_parser(value_parser!(String)),
        )
        .arg(
            Arg::new("deploy_sys_files")
                .long("deploy-sys-files")
                .env("DOD_DEPLOY_SYS_FILES")
                .value_parser(FalseyValueParser::new().map(|b| -> u8 {
                    if b { 1 } else { 0 }
                }))
                .action(ArgAction::Count)
                .help("Deploy files to directories other than the user's HOME [default]"),
        )
        .arg(
            Arg::new("no_deploy_sys_files")
                .long("no-deploy-sys-files")
                .value_parser(FalseyValueParser::new().map(|b| -> u8 {
                    if b { 1 } else { 0 }
                }))
                .action(ArgAction::Count)
                .help("Don't deploy files to directories other than the user's HOME"),
        )
        .arg(
            Arg::new("install_pkg_cmd")
                .long("install-pkg-cmd")
                .env("DOD_INSTALL_PKG_CMD")
                .value_delimiter(' ')
                .value_parser(value_parser!(OsString))
                .num_args(0..),
        )
        .arg(
            Arg::new("remove_pkg_cmd")
                .long("remove-pkg-cmd")
                .env("DOD_REMOVE_PKG_CMD")
                .value_delimiter(' ')
                .value_parser(value_parser!(OsString))
                .num_args(0..),
        )
        .arg(
            Arg::new("user_store")
                .long("user-store")
                .env("DOD_USER_STORE")
                .value_parser(value_parser!(PathBuf)),
        )
        .arg(
            Arg::new("verbosity")
                .short('v')
                .long("verbose")
                .global(true)
                .env("DOD_VERBOSE")
                .action(ArgAction::Count)
                .help("Verbosity level (-v = debug, -vv = trace)"),
        )
        .arg(
            Arg::new("logs_dir")
                .long("logs-dir")
                .env("DOD_LOGS_DIR")
                .value_parser(value_parser!(PathBuf)),
        )
        .arg(
            Arg::new("logs_max")
                .long("logs-max")
                .env("DOD_LOGS_MAX")
                .value_parser(value_parser!(usize)),
        );

    // --
    // * Add subcommands

    // --
    // * deploy

    cmd.subcommand(
        Command::new("deploy")
            .about("Deploy system configuration or specific modules")
            .arg(
                Arg::new("modules")
                    .value_name("MODULE")
                    .num_args(1..)
                    .value_parser(value_parser!(String)),
            )
            .arg(
                Arg::new("host")
                    .long("host")
                    .action(ArgAction::SetTrue)
                    .conflicts_with("modules")
                    .help("Deploy the host module"),
            ),
    )
    // --
    // * remove
       .subcommand(
           Command::new("remove")
               .about("Remove deployed modules and restore backups")
               .arg(
                   Arg::new("modules")
                       .value_name("MODULE")
                       .num_args(1..)
                       .value_parser(value_parser!(String)),
               )
               .arg(
                   Arg::new("host")
                       .long("host")
                       .action(ArgAction::SetTrue)
                       .conflicts_with("modules")
                       .help("Remove the host module"),
               ),
       )
    // --
    // * update
       .subcommand(
           Command::new("update")
               .about("Update module content")
               .arg(
                   Arg::new("modules")
                       .value_name("MODULE")
                       .num_args(1..)
                       .value_parser(value_parser!(String)),
               ),
       )
    // --
    // * sync
       .subcommand(
           Command::new("sync")
               .about("Synchronize modules")
               .arg(
                   Arg::new("components")
                       .value_name("COMPONENT")
                       .required(true)
                       .num_args(1..)
                       .value_parser(EnumValueParser::<SyncComponent>::new())
                       .help("Components to sync"),
               )
               .arg(
                   Arg::new("host")
                       .long("host")
                       .action(ArgAction::SetTrue)
                       .conflicts_with("modules")
                       .help("Sync the host module"),
               )
               .arg(
                   Arg::new("show_messages")
                       .long("show-messages")
                       .action(ArgAction::SetTrue)
                       .help("Show deployment messages"),
               )
               .arg(
                   Arg::new("modules")
                       .value_name("MODULE")
                       .num_args(1..)
                       .last(true)
                       .value_parser(value_parser!(String))
                       .help("Optional list of module names to sync"),
               ),
       )
    // --
    // * validate
       .subcommand(
           Command::new("validate")
               .about("Validate deployment state and check for differences")
               .arg(
                   Arg::new("diff")
                       .long("diff")
                       .action(ArgAction::SetTrue)
                       .help("Show detailed differences between source and deployed files"),
               )
               .arg(
                   Arg::new("fix")
                       .long("fix")
                       .action(ArgAction::SetTrue)
                       .help("Enter interactive fix mode for discrepancies"),
               ),
       )
    // --
    // * lookup
       .subcommand(
           Command::new("lookup")
               .about("Lookup source file")
               .arg(
                   Arg::new("file")
                       .required(true)
                       .value_parser(value_parser!(PathBuf))
                       .help("File path to lookup"),
               ),
       )
    // --
    // * uninstall
       .subcommand(Command::new("uninstall").about("Complete uninstall of dotdeploy"))
    // --
    // * completions
       .subcommand(
           Command::new("completions")
               .about("Generate shell completions")
               .arg(
                   Arg::new("shell")
                       .required(true)
                       .long("shell")
                       .short('s')
                       .value_parser(value_parser!(Shell))
                       .help("Set the shell for generating completions [values: bash, elvish, fish, powerShell, zsh]"),
               )
               .arg(
                   Arg::new("out")
                       .long("out")
                       .value_parser(value_parser!(PathBuf))
                       .help("Set the out directory for writing completions file"),
               ),
       )
}

// -------------------------------------------------------------------------------------------------
// CLI Commands
// -------------------------------------------------------------------------------------------------

/// Represents parsed command-line subcommands and their arguments
///
/// Contains variants for each supported operation with their respective options. Produced by
/// parsing raw CLI arguments using clap's ArgMatches structure.
#[derive(Debug)]
pub(crate) enum Commands {
    Deploy {
        modules: Option<Vec<String>>,
        host: bool,
    },
    Remove {
        modules: Option<Vec<String>>,
        host: bool,
    },
    Update {
        modules: Option<Vec<String>>,
    },
    Sync {
        components: Vec<SyncComponent>,
        host: bool,
        show_messages: bool,
        modules: Option<Vec<String>>,
    },
    Validate,
    Lookup {
        file: PathBuf,
    },
    Uninstall,
    Completions {
        shell: Shell,
        out: Option<PathBuf>,
    },
}

impl Commands {
    /// Converts raw CLI matches into structured Commands enum
    ///
    /// Acts as bridge between clap's ArgMatches structure and application logic.
    pub(crate) fn parse_command(matches: &clap::ArgMatches) -> Self {
        match matches.subcommand() {
            Some(("deploy", deploy_matches)) => Commands::Deploy {
                modules: deploy_matches
                    .get_many::<String>("modules")
                    .map(|v| v.cloned().collect()),
                host: deploy_matches.get_flag("host"),
            },
            Some(("remove", remove_matches)) => Commands::Remove {
                modules: remove_matches
                    .get_many::<String>("modules")
                    .map(|v| v.cloned().collect()),
                host: remove_matches.get_flag("host"),
            },
            Some(("update", update_matches)) => Commands::Update {
                modules: update_matches
                    .get_many::<String>("modules")
                    .map(|v| v.cloned().collect()),
            },
            Some(("sync", sync_matches)) => Commands::Sync {
                components: sync_matches
                    .get_many::<SyncComponent>("components")
                    .map(|v| v.cloned().collect())
                    .unwrap_or_default(),
                host: sync_matches.get_flag("host"),
                show_messages: sync_matches.get_flag("show-messages"),
                modules: sync_matches
                    .get_many::<String>("modules")
                    .map(|v| v.cloned().collect()),
            },
            Some(("validate", _)) => Commands::Validate,
            Some(("lookup", lookup_matches)) => Commands::Lookup {
                file: lookup_matches.get_one::<PathBuf>("file").unwrap().clone(),
            },
            Some(("uninstall", _)) => Commands::Uninstall,
            Some(("completions", completions_matches)) => Commands::Completions {
                shell: *completions_matches.get_one::<Shell>("shell").unwrap(),
                out: completions_matches.get_one::<PathBuf>("out").cloned(),
            },
            // Default case, should never happen with clap validation
            _ => unreachable!(),
        }
    }
}

// -------------------------------------------------------------------------------------------------
// Flag parser
// -------------------------------------------------------------------------------------------------

/// Determines effective state of conflicting boolean flags with environment fallback
///
/// Resolves precedence between mutually exclusive flags (e.g. --use-sudo vs --no-use-sudo) by
/// considering:
///
/// * Last specified flag on command line
/// * Environment variable default
/// * Returns None if no relevant options were specified
pub(crate) fn flag_is_enabled(matches: &ArgMatches, on_flag: &str, off_flag: &str) -> Option<bool> {
    // Determine the name the raw flags, following the "FLAG"/"no-FLAG" pattern
    let raw_on_flag = ["--", &on_flag.replace("_", "-")].join("");
    let raw_off_flag = ["--", &off_flag.replace("_", "-")].join("");

    // Get raw arguments to determine the order
    let raw_args: Vec<String> = env::args().collect();

    // Find the last occurrence of either --FLAG or --no-FLAG
    let mut last_on_position = -1;
    let mut last_off_position = -1;

    for (index, arg) in raw_args.iter().enumerate() {
        if arg == raw_on_flag.as_str() {
            last_on_position = index as i32;
        } else if arg == raw_off_flag.as_str() {
            last_off_position = index as i32;
        }
    }

    if last_on_position > last_off_position {
        Some(true)
    } else if last_off_position > last_on_position {
        Some(false)
    } else {
        // Neither was specified on command line, check environment variable
        let var = matches.get_count(on_flag) > 0;
        if var { Some(var) } else { None }
    }
}

// -------------------------------------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_cli() {
        build_cli().debug_assert();
    }
}
