use std::fmt::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;

use anyhow::{Context, Result};
use log::error;

#[macro_use]
extern crate log;

mod cli;
mod config;
mod logs;
mod store;
mod utils;

// -------------------------------------------------------------------------------------------------
// Global Variables
// -------------------------------------------------------------------------------------------------

/// Global variable, available to all threads, indicating if sudo can be used.
static USE_SUDO: LazyLock<AtomicBool> = LazyLock::new(|| AtomicBool::new(false));

fn main() {
    let dotdeploy_config = match init_config() {
        Ok(config) => config,
        Err(e) => {
            eprint!("Failed to initialize config. Exiting");
            eprint!("{}", e);
            std::process::exit(1);
        }
    };
    match run(dotdeploy_config) {
        Ok(success) if success => std::process::exit(0),
        Ok(_) => std::process::exit(1),
        Err(e) => {
            display_error(e);
            std::process::exit(1);
        }
    }
}

/// Formats and displays an error chain in a user-friendly way.
///
/// Takes an anyhow::Error and displays its full error chain, showing each cause in sequence. The
/// output format is:
///
/// ```
/// <main error message>
/// Caused by:
///     <cause 1>
///     <cause 2>
///     ...
/// ```
pub(crate) fn display_error(error: anyhow::Error) {
    let mut chain = error.chain();
    let mut error_message = format!("{}\nCaused by:\n", chain.next().unwrap());

    for e in chain {
        writeln!(error_message, "    {}", e).unwrap();
    }
    // Remove last \n
    error_message.pop();

    error!("{}", error_message);
}

/// Initializes and configures Dotdeploy by:
/// 1. Parsing CLI arguments
/// 2. Setting up logging
/// 3. Loading configuration from file (if present)
/// 4. Merging CLI arguments with file configuration
/// 5. Setting environment variables for (nearly) all configuration values
/// 6. Returning the final configuration
///
/// # Returns
///
/// A `Result` containing the fully initialized `DotdeployConfig` or an error if initialization
/// failed.
///
/// # Errors
///
/// Returns an error if:
/// - Logging initialization fails
/// - Configuration file parsing fails
/// - Required paths or values are missing
fn init_config() -> Result<config::DotdeployConfig> {
    let cli = cli::get_cli();
    logs::init_logging(cli.verbosity)?;

    // Read config from file, if any
    let mut dotdeploy_config =
        config::DotdeployConfig::init().context("Failed to initialize Dotdeploy config")?;

    // Merge CLI args into config
    if let Some(flag) = cli.dry_run {
        dotdeploy_config.dry_run = flag;
    }
    if let Some(flag) = cli.force {
        dotdeploy_config.force = flag;
    }
    if let Some(flag) = cli.noconfirm {
        dotdeploy_config.noconfirm = flag;
    }
    if let Some(path) = cli.config_root {
        dotdeploy_config.config_root = path;
    }
    if let Some(path) = cli.modules_root {
        dotdeploy_config.modules_root = path;
    }
    if let Some(path) = cli.hosts_root {
        dotdeploy_config.hosts_root = path;
    }
    if let Some(name) = cli.hostname {
        dotdeploy_config.hostname = name;
    }
    if let Some(name) = cli.distribution {
        dotdeploy_config.distribution = name;
    }
    if let Some(flag) = cli.use_sudo {
        dotdeploy_config.use_sudo = flag;
    }
    if let Some(flag) = cli.deploy_sys_files {
        dotdeploy_config.deploy_sys_files = flag;
    }
    if cli.install_pkg_cmd.is_some() {
        dotdeploy_config.install_pkg_cmd = cli.install_pkg_cmd;
    }
    if cli.remove_pkg_cmd.is_some() {
        dotdeploy_config.remove_pkg_cmd = cli.remove_pkg_cmd;
    }
    if let Some(flag) = cli.skip_pkg_install {
        dotdeploy_config.skip_pkg_install = flag;
    }
    if let Some(path) = cli.user_store {
        dotdeploy_config.user_store_path = path;
    }
    if let Some(path) = cli.system_store {
        dotdeploy_config.system_store_path = path;
    }

    // Make config available as environment variables
    unsafe {
        std::env::set_var("DOD_DRY_RUN", &dotdeploy_config.dry_run.to_string());
        std::env::set_var("DOD_FORCE", &dotdeploy_config.force.to_string());
        std::env::set_var("DOD_YES", &dotdeploy_config.noconfirm.to_string());
        std::env::set_var("DOD_CONFIG_ROOT", &dotdeploy_config.config_root);
        std::env::set_var("DOD_MODULES_ROOT", &dotdeploy_config.modules_root);
        std::env::set_var("DOD_HOSTS_ROOT", &dotdeploy_config.hosts_root);
        std::env::set_var("DOD_HOSTNAME", &dotdeploy_config.hostname);
        std::env::set_var("DOD_DISTRIBUTION", &dotdeploy_config.distribution);
        std::env::set_var("DOD_USE_SUDO", &dotdeploy_config.use_sudo.to_string());
        std::env::set_var(
            "DOD_DEPLOY_SYS_FILES",
            &dotdeploy_config.deploy_sys_files.to_string(),
        );
        std::env::set_var(
            "DOD_SKIP_PKG_INSTALL",
            &dotdeploy_config.skip_pkg_install.to_string(),
        );
        std::env::set_var("DOD_USER_STORE", &dotdeploy_config.user_store_path);
        std::env::set_var("DOD_SYSTEM_STORE", &dotdeploy_config.system_store_path);
    }

    // Set USE_SUDO
    USE_SUDO.store(dotdeploy_config.use_sudo, Ordering::Relaxed);

    debug!("Config initialized:\n{:#?}", &dotdeploy_config);

    Ok(dotdeploy_config)
}

#[tokio::main]
async fn run(config: config::DotdeployConfig) -> Result<bool> {
    // store::create_system_dir(&config.system_store_path).await?;
    // store::create_user_dir(&config.user_store_path).await?;
    todo!("oha")
}
