use color_eyre::{eyre::WrapErr, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, OnceLock, RwLock};
use tracing::{debug, instrument};

mod cli;
mod config;
mod logs;
// mod modules;
mod store;
mod utils;

// -------------------------------------------------------------------------------------------------
// Global Variables
// -------------------------------------------------------------------------------------------------

/// Global flag indicating whether sudo/privilege elevation should be used.
///
/// This is set during initialization based on configuration and used throughout the application to
/// determine if operations need elevated privileges.
static USE_SUDO: LazyLock<AtomicBool> = LazyLock::new(|| AtomicBool::new(false));

/// Global lock to synchronize terminal access for privilege elevation prompts.
///
/// This ensures that sudo password prompts and similar terminal interactions don't overlap and
/// confuse the user, especially in concurrent operations.
static TERMINAL_LOCK: LazyLock<Arc<RwLock<()>>> = LazyLock::new(|| Arc::new(RwLock::new(())));

/// Global storage for the sudo command to be used for privilege elevation.
///
/// This is initialized during startup based on configuration and stores the specific command (e.g.,
/// "sudo", "doas") that should be used when elevated privileges are needed.
static SUDO_CMD: OnceLock<String> = OnceLock::new();

#[instrument]
fn main() {
    // Initialize color_eyre
    color_eyre::install().unwrap_or_else(|e| panic!("Failed to initialize color_eyre: {:?}", e));

    // Initialize logging
    let logger = match logs::LoggerBuilder::new()
        .with_verbosity(cli::get_cli().verbosity)
        .build()
    {
        Ok(logger) => logger,
        Err(e) => {
            eprintln!("Failed to setup logging. Exiting");
            eprintln!("{:?}", e);
            std::process::exit(1);
        }
    };
    let _log_guard = match logger.start() {
        Ok(guard) => guard,
        Err(e) => {
            eprintln!("Failed to initialize logging. Exiting");
            eprintln!("{:?}", e);
            std::process::exit(1);
        }
    };

    let dotdeploy_config = match init_config() {
        Ok(config) => config,
        Err(e) => {
            eprintln!("Failed to initialize config. Exiting");
            eprintln!("{:?}", e);
            std::process::exit(1);
        }
    };

    match run(dotdeploy_config) {
        Ok(success) if success => std::process::exit(0),
        Ok(_) => std::process::exit(1),
        Err(e) => {
            eprintln!("An error occured during deployment. Exiting");
            eprintln!("{:?}", e);
            std::process::exit(1);
        }
    }
}

/// Initializes and configures Dotdeploy by:
///
/// 1. Parsing CLI arguments
/// 2. Loading configuration from file (if present)
/// 3. Merging CLI arguments with file configuration
/// 4. Setting environment variables for (nearly) all configuration values
/// 5. Returning the final configuration
///
/// # Errors
///
/// Returns an error if:
/// - Configuration file parsing fails
/// - Required paths or values are missing
#[instrument]
fn init_config() -> Result<config::DotdeployConfig> {
    let cli = cli::get_cli();

    // Read config from file, if any
    let mut dotdeploy_config =
        config::DotdeployConfig::init().wrap_err("Failed to initialize Dotdeploy config")?;

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
    if let Some(path) = cli.dotfiles_root {
        dotdeploy_config.dotfiles_root = path;
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
        std::env::set_var("DOD_DRY_RUN", dotdeploy_config.dry_run.to_string());
        std::env::set_var("DOD_FORCE", dotdeploy_config.force.to_string());
        std::env::set_var("DOD_YES", dotdeploy_config.noconfirm.to_string());
        std::env::set_var("DOD_DOTFILES_ROOT", &dotdeploy_config.dotfiles_root);
        std::env::set_var("DOD_MODULES_ROOT", &dotdeploy_config.modules_root);
        std::env::set_var("DOD_HOSTS_ROOT", &dotdeploy_config.hosts_root);
        std::env::set_var("DOD_HOSTNAME", &dotdeploy_config.hostname);
        std::env::set_var("DOD_DISTRIBUTION", &dotdeploy_config.distribution);
        std::env::set_var("DOD_USE_SUDO", dotdeploy_config.use_sudo.to_string());
        std::env::set_var(
            "DOD_DEPLOY_SYS_FILES",
            dotdeploy_config.deploy_sys_files.to_string(),
        );
        std::env::set_var(
            "DOD_SKIP_PKG_INSTALL",
            dotdeploy_config.skip_pkg_install.to_string(),
        );
        std::env::set_var("DOD_USER_STORE", &dotdeploy_config.user_store_path);
        std::env::set_var("DOD_SYSTEM_STORE", &dotdeploy_config.system_store_path);
    }

    // Set USE_SUDO
    USE_SUDO.store(dotdeploy_config.use_sudo, Ordering::Relaxed);
    if USE_SUDO.load(Ordering::Relaxed) {
        let _ = SUDO_CMD.set(dotdeploy_config.sudo_cmd.clone());
    }

    debug!("Config initialized:\n{:#?}", &dotdeploy_config);

    Ok(dotdeploy_config)
}

#[tokio::main]
#[instrument]
async fn run(config: config::DotdeployConfig) -> Result<bool> {
    let stores = store::Stores::new(&config)
        .await
        .wrap_err("Failed to initialize stores")?;
    debug!(stores = ?stores, "Stores initialized");

    stores.user_store.pool.close().await;
    if let Some(sys_store) = stores.system_store {
        sys_store.pool.close().await
    }

    Ok(true)
}
