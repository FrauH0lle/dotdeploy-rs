use color_eyre::eyre::{OptionExt, WrapErr, eyre};
use color_eyre::{Result, Section};
use config::DotdeployConfigBuilder;
use handlebars::Handlebars;
use logs::Logger;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use toml::Value;
use tracing::{debug, instrument};
use utils::file_fs;

mod cli;
mod cmds;
mod config;
mod errors;
mod handlebars_helper;
mod logs;
mod modules;
mod phases;
mod store;
#[cfg(test)]
mod tests;
mod utils;

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

    let (tx, rx) = mpsc::channel();

    match run(dotdeploy_config, logger, rx) {
        Ok((success, loop_running)) => {
            if loop_running {
                let _ = tx.send(());
                std::thread::sleep(std::time::Duration::from_millis(600));
            }
            match success {
                true => std::process::exit(0),
                _ => std::process::exit(1),
            }
        }
        Err(e) => {
            eprintln!("An error occured during deployment. Exiting");
            eprintln!("{:?}", e);
            let _ = tx.send(());
            std::thread::sleep(std::time::Duration::from_millis(600));
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

    Ok(dotdeploy_config)
}

/// Make config available as environment variables.
///
/// For nearly all configuration values, a corresponding environment variable will be set.
fn export_env_vars(dotdeploy_config: &config::DotdeployConfig) -> Result<()> {
    let (name, version) = dotdeploy_config
        .distribution
        .split_once(':')
        .ok_or_else(|| {
            eyre!(
                "Invalid distribution name format: {}",
                dotdeploy_config.distribution
            )
        })?;

    unsafe {
        std::env::set_var("DOD_DRY_RUN", dotdeploy_config.dry_run.to_string());
        std::env::set_var("DOD_FORCE", dotdeploy_config.force.to_string());
        std::env::set_var("DOD_YES", dotdeploy_config.noconfirm.to_string());
        std::env::set_var("DOD_DOTFILES_ROOT", &dotdeploy_config.dotfiles_root);
        std::env::set_var("DOD_MODULES_ROOT", &dotdeploy_config.modules_root);
        std::env::set_var("DOD_HOSTS_ROOT", &dotdeploy_config.hosts_root);
        std::env::set_var("DOD_HOSTNAME", &dotdeploy_config.hostname);
        std::env::set_var("DOD_DISTRIBUTION", &dotdeploy_config.distribution);
        std::env::set_var("DOD_DISTRIBUTION_NAME", name);
        std::env::set_var("DOD_DISTRIBUTION_VERSION", version);
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

    Ok(())
}

/// Make config available as environment variables.
///
/// For nearly all configuration values, a corresponding environment variable will be set.
fn setup_context(dotdeploy_config: &config::DotdeployConfig) -> Result<HashMap<String, Value>> {
    let (name, version) = dotdeploy_config
        .distribution
        .split_once(':')
        .ok_or_else(|| {
            eyre!(
                "Invalid distribution name format: {}",
                dotdeploy_config.distribution
            )
        })?;

    let mut context: HashMap<String, Value> = HashMap::new();
    context.insert(
        "DOD_DOTFILES_ROOT".to_string(),
        Value::String(file_fs::path_to_string(&dotdeploy_config.dotfiles_root)?),
    );
    context.insert(
        "DOD_MODULES_ROOT".to_string(),
        Value::String(file_fs::path_to_string(&dotdeploy_config.modules_root)?),
    );
    context.insert(
        "DOD_HOSTS_ROOT".to_string(),
        Value::String(file_fs::path_to_string(&dotdeploy_config.hosts_root)?),
    );
    context.insert(
        "DOD_HOSTNAME".to_string(),
        Value::String(dotdeploy_config.hostname.to_string()),
    );
    context.insert(
        "DOD_DISTRIBUTION".to_string(),
        Value::String(dotdeploy_config.distribution.to_string()),
    );
    context.insert(
        "DOD_DISTRIBUTION_NAME".to_string(),
        Value::String(name.to_string()),
    );
    context.insert(
        "DOD_DISTRIBUTION_VERSION".to_string(),
        Value::String(version.to_string()),
    );
    context.insert(
        "DOD_USE_SUDO".to_string(),
        Value::String(dotdeploy_config.use_sudo.to_string()),
    );
    context.insert(
        "DOD_DEPLOY_SYS_FILES".to_string(),
        Value::String(dotdeploy_config.deploy_sys_files.to_string()),
    );
    context.insert(
        "DOD_USER_STORE".to_string(),
        Value::String(file_fs::path_to_string(&dotdeploy_config.user_store_path)?),
    );
    context.insert(
        "DOD_SYSTEM_STORE".to_string(),
        Value::String(file_fs::path_to_string(
            &dotdeploy_config.system_store_path,
        )?),
    );

    Ok(context)
}

#[tokio::main]
async fn run(
    config: config::DotdeployConfig,
    logger: Logger,
    rx: mpsc::Receiver<()>,
) -> Result<(bool, bool)> {
    // --
    // * Setup

    // Export environment variables to process
    export_env_vars(&config)?;

    // Initialize privilege manager
    let pm = Arc::new(
        utils::sudo::PrivilegeManagerBuilder::new()
            .with_use_sudo(config.use_sudo)
            .with_root_cmd(match config.sudo_cmd.as_str() {
                "sudo" => utils::sudo::GetRootCmd::use_sudo(),
                "doas" => utils::sudo::GetRootCmd::use_doas(),
                _ => {
                    return Err(eyre!("Unsupported privilege elevation command")
                        .suggestion("Check the value of 'sudo_cmd' in the dotdeploy config"));
                }
            })
            .with_terminal_lock(logger.terminal_lock)
            .with_channel_rx(Some(rx))
            .build()?,
    );

    // Initialize stores
    let stores = Arc::new(
        store::Stores::new(&config, Arc::clone(&pm))
            .await
            .wrap_err("Failed to initialize stores")?,
    );
    debug!(stores = ?stores, "Stores initialized");

    // Initialize handlebars templating
    let mut handlebars: Handlebars<'static> = Handlebars::new();
    handlebars.set_strict_mode(true);
    handlebars.register_helper("contains", Box::new(handlebars_helper::contains_helper));
    // Set up the context used by handlebars
    let context = setup_context(&config)?;

    // Wrap config in an Arc as it will be shared across threads
    let config = Arc::new(config);

    // Get CLI parameters
    let cli = cli::get_cli();

    // --
    // * Execute

   let cmd_result = match cli.command {
        cli::Commands::Deploy { modules } => {
            let modules = modules.unwrap_or_else(|| {
                // IF no modules are given, assume host module
                let host_module = vec![
                    config.hosts_root.display().to_string().clone(),
                    config.hostname.clone(),
                ]
                .join(std::path::MAIN_SEPARATOR_STR);
                vec![host_module]
            });
            cmds::deploy::deploy(
                modules,
                config,
                Arc::clone(&stores),
                context,
                handlebars,
                Arc::clone(&pm),
            )
            .await
        }
        cli::Commands::Remove { modules } => todo!(),
        cli::Commands::Update { packages } => todo!(),
        cli::Commands::Sync { auto } => todo!(),
        cli::Commands::Validate { diff, fix } => todo!(),
        cli::Commands::Nuke { really } => todo!(),
    };

    // --
    // * Shutdown

    let vacuum = sqlx::query!("VACUUM");
    vacuum.execute(&stores.user_store.pool).await?;
    stores.user_store.pool.close().await;
    if let Some(sys_store) = &stores.system_store {
        let vacuum = sqlx::query!("VACUUM");
        vacuum.execute(&sys_store.pool).await?;
        sys_store.pool.close().await
    }

    let loop_running = pm.loop_running.load(Ordering::Relaxed);
    Ok((cmd_result?, loop_running))
}
