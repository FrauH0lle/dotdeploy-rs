use color_eyre::{Result, eyre::WrapErr};
use config::DotdeployConfigBuilder;
use logs::Logger;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, OnceLock, RwLock};
use tracing::{debug, instrument};
use utils::sudo::PrivilegeManager;

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

    let dotdeploy_config = match init_config() {
        Ok(config) => config,
        Err(e) => {
            eprintln!("Failed to initialize config. Exiting");
            eprintln!("{:?}", e);
            std::process::exit(1);
        }
    };

    // Initialize logging
    let logger = match logs::LoggerBuilder::new()
        .with_verbosity(cli::get_cli().verbosity)
        .with_log_dir(&dotdeploy_config.logs_dir)
        .with_max_logs(dotdeploy_config.logs_max)
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

    debug!("Config initialized:\n{:#?}", &dotdeploy_config);

    export_env_vars(&dotdeploy_config);

    match run(dotdeploy_config, logger) {
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
/// 4. Returning the final configuration
///
/// # Errors
///
/// Returns an error if:
/// - Configuration file parsing fails
/// - Required paths or values are missing
fn init_config() -> Result<config::DotdeployConfig> {
    let cli = cli::get_cli();

    // Initialize config and merge CLI args into config
    let dotdeploy_config = DotdeployConfigBuilder::new()
        .with_dry_run(cli.dry_run)
        .with_force(cli.force)
        .with_noconfirm(cli.noconfirm)
        .with_config_root(cli.config_root)
        .with_dotfiles_root(cli.dotfiles_root)
        .with_modules_root(cli.modules_root)
        .with_hosts_root(cli.hosts_root)
        .with_hostname(cli.hostname)
        .with_distribution(cli.distribution)
        .with_use_sudo(cli.use_sudo)
        .with_deploy_sys_files(cli.deploy_sys_files)
        .with_install_pkg_cmd(cli.install_pkg_cmd)
        .with_remove_pkg_cmd(cli.remove_pkg_cmd)
        .with_skip_pkg_install(cli.skip_pkg_install)
        .with_user_store_path(cli.user_store)
        .with_system_store_path(cli.system_store)
        .with_logs_dir(cli.logs_dir)
        .with_logs_max(cli.logs_max)
        .build(cli.verbosity)?;

    // Set USE_SUDO
    USE_SUDO.store(dotdeploy_config.use_sudo, Ordering::Relaxed);
    if USE_SUDO.load(Ordering::Relaxed) {
        let _ = SUDO_CMD.set(dotdeploy_config.sudo_cmd.clone());
    }

    Ok(dotdeploy_config)
}

/// Make config available as environment variables.
///
/// For nearly all configuration values, a corresponding environment variable will be set.
fn export_env_vars(dotdeploy_config: &config::DotdeployConfig) {
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
}

#[tokio::main]
#[instrument]
async fn run(config: config::DotdeployConfig, logger: Logger) -> Result<bool> {
    let privilege_ctx = utils::sudo::PrivilegeManagerBuilder::new()
        .with_use_sudo(config.use_sudo)
        .with_root_cmd(match config.sudo_cmd.as_str() {
            "sudo" => utils::sudo::GetRootCmd::use_sudo(),
            "doas" => utils::sudo::GetRootCmd::use_doas(),
            _ => unreachable!("Unsupported privilege elevation command"),
        })
        .with_terminal_lock(logger.terminal_lock)
        .build()?;

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
