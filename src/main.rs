use crate::store::Store;
use clap::ArgMatches;
use cmds::sync::{SyncCtx, SyncOp};
use color_eyre::eyre::{WrapErr, eyre};
use color_eyre::{Result, Section};
use config::DotdeployConfigBuilder;
use handlebars::Handlebars;
use logs::Logger;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use toml::Value;
use tracing::{debug, error, info};

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

fn main() {
    // Initialize color_eyre
    color_eyre::install().unwrap_or_else(|e| panic!("Failed to initialize color_eyre: {:?}", e));

    let cli_matches = cli::build_cli().get_matches();

    let dotdeploy_config = match init_config(&cli_matches) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("Failed to initialize config. Exiting");
            eprintln!("{:?}", e);
            std::process::exit(1);
        }
    };

    // Initialize logging
    let logger = match logs::LoggerBuilder::default()
        .with_verbosity(cli_matches.get_count("verbositiy").min(2))
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
                std::thread::sleep(std::time::Duration::from_millis(210));
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
            std::thread::sleep(std::time::Duration::from_millis(210));
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
fn init_config(cli: &ArgMatches) -> Result<config::DotdeployConfig> {
    // Initialize config and merge CLI args into config
    let dotdeploy_config = DotdeployConfigBuilder::default()
        .with_dry_run(Some(false))
        .with_force(cli::flag_is_enabled(cli, "force", "no_force"))
        .with_noconfirm(cli::flag_is_enabled(cli, "no_ask", "ask"))
        .with_config_file(cli.get_one::<PathBuf>("config_file").cloned())
        .with_dotfiles_root(cli.get_one::<PathBuf>("dotfiles_root").cloned())
        .with_modules_root(cli.get_one::<PathBuf>("modules_root").cloned())
        .with_hosts_root(cli.get_one::<PathBuf>("hosts_root").cloned())
        .with_hostname(cli.get_one::<String>("hostname").cloned())
        .with_distribution(cli.get_one::<String>("distribution").cloned())
        .with_use_sudo(cli::flag_is_enabled(cli, "use_sudo", "no_use_sudo"))
        .with_sudo_cmd(cli.get_one::<String>("sudo_cmd").cloned())
        .with_deploy_sys_files(cli::flag_is_enabled(
            cli,
            "deploy_sys_files",
            "no_deploy_sys_files",
        ))
        .with_install_pkg_cmd(
            cli.get_many::<OsString>("install_pkg_cmd")
                .map(|v| v.cloned().collect()),
        )
        .with_remove_pkg_cmd(
            cli.get_many::<OsString>("remove_pkg_cmd")
                .map(|v| v.cloned().collect()),
        )
        .with_user_store_path(cli.get_one::<PathBuf>("user_store").cloned())
        .with_logs_dir(cli.get_one::<PathBuf>("logs_dir").cloned())
        .with_logs_max(cli.get_one::<usize>("logs_max").copied())
        .build(cli.get_count("verbosity").min(2))?;

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
        std::env::set_var("DOD_USER_STORE", &dotdeploy_config.user_store_path);
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
        Value::String(
            dotdeploy_config
                .dotfiles_root
                .to_str()
                .ok_or_else(|| eyre!("{:?} is not valid UTF-8", dotdeploy_config.dotfiles_root))?
                .to_string(),
        ),
    );
    context.insert(
        "DOD_MODULES_ROOT".to_string(),
        Value::String(
            dotdeploy_config
                .modules_root
                .to_str()
                .ok_or_else(|| eyre!("{:?} is not valid UTF-8", dotdeploy_config.modules_root))?
                .to_string(),
        ),
    );
    context.insert(
        "DOD_HOSTS_ROOT".to_string(),
        Value::String(
            dotdeploy_config
                .hosts_root
                .to_str()
                .ok_or_else(|| eyre!("{:?} is not valid UTF-8", dotdeploy_config.hosts_root))?
                .to_string(),
        ),
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
        Value::String(
            dotdeploy_config
                .user_store_path
                .to_str()
                .ok_or_else(|| eyre!("{:?} is not valid UTF-8", dotdeploy_config.user_store_path))?
                .to_string(),
        ),
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

    // Get CLI
    let mut cmd = cli::build_cli();
    let arg_matches = cli::build_cli().get_matches();

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

    // Initialize store
    let store = Arc::new(
        store::sqlite::init_sqlite_store(&config, Arc::clone(&pm))
            .await
            .wrap_err("Failed to initialize user store")?,
    );
    debug!(store = ?store, "Store initialized");

    // --
    // * Initialize handlebars templating

    let mut handlebars: Handlebars<'static> = Handlebars::new();
    handlebars.set_strict_mode(true);
    handlebars_misc_helpers::register(&mut handlebars);
    handlebars.register_helper("contains", Box::new(handlebars_helper::contains_helper));
    handlebars.register_helper(
        "is_executable",
        Box::new(handlebars_helper::is_executable_helper),
    );
    handlebars.register_helper(
        "find_executable",
        Box::new(handlebars_helper::find_executable_helper),
    );
    handlebars.register_helper(
        "command_success",
        Box::new(handlebars_helper::command_success_helper),
    );
    handlebars.register_helper(
        "command_output",
        Box::new(handlebars_helper::command_output_helper),
    );

    // Set up the context used by handlebars
    let context = setup_context(&config)?;

    // Wrap config in an Arc as it will be shared across threads
    let config = Arc::new(config);

    // --
    // * Execute

    // If we are performing an uninstallation.
    let mut is_uninstall = false;

    let cmd_result = match cli::Commands::parse_command(&arg_matches) {
        cli::Commands::Deploy { modules, host } => {
            let modules =
                get_selected_modules(host, false, modules, &config, Arc::clone(&store)).await?;

            if modules.is_empty() {
                error!("No modules specified");
                return Ok((false, false));
            }

            let config = Arc::clone(&config);

            cmds::sync::sync(
                modules,
                SyncCtx {
                    config,
                    components: vec![
                        cli::SyncComponent::Files,
                        cli::SyncComponent::Tasks,
                        cli::SyncComponent::Packages,
                    ],
                    store: Arc::clone(&store),
                    context,
                    handlebars,
                    pm: Arc::clone(&pm),
                },
                SyncOp::Deploy,
                true,
            )
            .await
        }
        cli::Commands::Remove { modules, host } => {
            let modules =
                get_selected_modules(host, false, modules, &config, Arc::clone(&store)).await?;

            if modules.is_empty() {
                error!("No modules specified");
                return Ok((false, false));
            }

            let config = Arc::clone(&config);

            cmds::remove::remove(
                modules,
                config,
                Arc::clone(&store),
                context,
                handlebars,
                Arc::clone(&pm),
            )
            .await
        }
        cli::Commands::Update { modules } => {
            let config = Arc::clone(&config);

            cmds::update::update(modules, config, Arc::clone(&store), Arc::clone(&pm)).await
        }
        cli::Commands::Lookup { file } => cmds::lookup::lookup(file, Arc::clone(&store)).await,
        cli::Commands::Sync {
            components,
            host,
            show_messages,
            modules,
        } => {
            let modules =
                get_selected_modules(host, true, modules, &config, Arc::clone(&store)).await?;

            if modules.is_empty() {
                error!("No modules specified or found in store");
                return Ok((false, false));
            }

            let config = Arc::clone(&config);

            cmds::sync::sync(
                modules,
                SyncCtx {
                    config,
                    components,
                    store: Arc::clone(&store),
                    context,
                    handlebars,
                    pm: Arc::clone(&pm),
                },
                SyncOp::Sync,
                show_messages,
            )
            .await
        }
        cli::Commands::Validate => todo!(),
        cli::Commands::Uninstall => {
            let modules = store
                .get_all_modules()
                .await?
                .into_iter()
                .map(|m| m.name)
                .collect();

            // Switch on uninstall flag
            is_uninstall = true;

            let config = Arc::clone(&config);

            cmds::remove::remove(
                modules,
                config,
                Arc::clone(&store),
                context,
                handlebars,
                Arc::clone(&pm),
            )
            .await
        }
        cli::Commands::Completions { shell, out } => {
            if let Some(out) = out {
                let name = cmd.get_name().to_string();
                clap_complete::generate_to(shell, &mut cmd, name, &out).wrap_err_with(|| {
                    format!(
                        "Failed to build completions for {} and write them to {}",
                        shell,
                        out.display()
                    )
                })?;
                Ok(true)
            } else {
                let name = cmd.get_name().to_string();
                clap_complete::generate(shell, &mut cmd, name, &mut std::io::stdout());
                Ok(true)
            }
        }
    };

    // --
    // * Pool shutdown

    let vacuum = sqlx::query!("VACUUM");
    vacuum.execute(&store.pool).await?;
    store.pool.close().await;

    // --
    // * Uninstall

    if is_uninstall && cmd_result.is_ok() {
        // Remove database
        if config.force
            || utils::common::ask_boolean(&format!(
                "{}\n{}",
                "Remove store database? [y/N]",
                "(You can skip this prompt with the CLI argument '-f/--force')",
            ))
        {
            info!("Removing {}", &store.path.display());
            tokio::fs::remove_file(&store.path).await?;
        }

        info!(
            "You can now safely delete\n{}",
            format!(
                " - {}\n - {}\n - {}\n",
                &config
                    .config_file
                    .parent()
                    .ok_or_else(|| eyre!(
                        "Failed to get parent of {}",
                        config.config_file.display()
                    ))?
                    .display(),
                &store
                    .path
                    .parent()
                    .ok_or_else(|| eyre!("Failed to get parent of {}", store.path.display()))?
                    .display(),
                &config.logs_dir.display()
            )
        )
    }

    let loop_running = pm.loop_running.load(Ordering::Relaxed);
    Ok((cmd_result?, loop_running))
}

async fn get_selected_modules(
    host: bool,
    collect: bool,
    modules: Option<Vec<String>>,
    config: &config::DotdeployConfig,
    store: Arc<store::sqlite::SQLiteStore>,
) -> Result<Vec<String>> {
    if host {
        Ok(vec![format!("hosts/{}", config.hostname)])
    } else if let Some(modules) = modules {
        Ok(modules)
    } else if collect {
        let mut deployed_modules = store.get_all_modules().await?;
        if !deployed_modules.is_empty() {
            deployed_modules.retain(|m| m.reason.as_str() == "manual");
            Ok(deployed_modules.into_iter().map(|m| m.name).collect())
        } else {
            Ok(vec![])
        }
    } else {
        Ok(vec![])
    }
}
