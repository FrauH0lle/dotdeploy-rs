use anyhow::{Context, Result};

use lazy_static::lazy_static;

use std::fmt::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[macro_use]
extern crate log;

mod cli;
mod config;
mod deploy;
mod modules;
mod packages;
mod phases;
mod phases2;
mod remove;
mod store;
mod utils;

use store::Stores;

lazy_static! {
    /// Global variable, available to all threads, indicating if the system store can be used.
    pub(crate) static ref DEPLOY_SYSTEM_FILES: AtomicBool = AtomicBool::new(false);
    /// Global variable, available to all threads, indicating if sudo can be used.
    pub(crate) static ref USE_SUDO: AtomicBool = AtomicBool::new(false);
}

fn main() {
    match run() {
        Ok(success) if success => std::process::exit(0),
        Ok(_) => std::process::exit(1),
        Err(e) => {
            display_error(e);
            std::process::exit(1);
        }
    }
}

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

#[tokio::main]
async fn run() -> Result<bool> {
    let cli = cli::get_cli();

    simplelog::TermLogger::init(
        match cli.verbosity {
            0 => simplelog::LevelFilter::Info,
            1 => simplelog::LevelFilter::Debug,
            2 => simplelog::LevelFilter::Trace,
            _ => unreachable!(),
        },
        simplelog::ConfigBuilder::new()
            .set_time_level(simplelog::LevelFilter::Debug)
            .set_location_level(simplelog::LevelFilter::Debug)
            .set_target_level(simplelog::LevelFilter::Debug)
            .set_thread_level(simplelog::LevelFilter::Debug)
            .set_level_padding(simplelog::LevelPadding::Left)
            .add_filter_allow("dotdeploy".to_string())
            .build(),
        simplelog::TerminalMode::Mixed,
        simplelog::ColorChoice::Auto,
    )
    .unwrap();

    // The Dotdeploy config should be on the top level as it contains information like the paths
    // which are needed often.
    let mut dotdeploy_config =
        config::DotdeployConfig::init().context("Failed to initialize Dotdeploy config")?;
    if cli.skip_pkg_install {
        dotdeploy_config.skip_pkg_install = cli.skip_pkg_install;
    }

    // Set global variables according to config
    DEPLOY_SYSTEM_FILES.store(dotdeploy_config.deploy_sys_files, Ordering::Relaxed);
    USE_SUDO.store(dotdeploy_config.use_sudo, Ordering::Relaxed);

    // Make config available as environment variables
    unsafe {
        std::env::set_var("DOD_ROOT", &dotdeploy_config.config_root);
        std::env::set_var("DOD_MODULES_ROOT", &dotdeploy_config.modules_root);
        std::env::set_var("DOD_HOSTS_ROOT", &dotdeploy_config.hosts_root);
        std::env::set_var("DOD_HOSTNAME", &dotdeploy_config.hostname);
        std::env::set_var("DOD_DISTRO", &dotdeploy_config.distribution);
    }

    trace!("Config values: {:#?}", &dotdeploy_config);

    // Handlebars templating
    let mut context: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    let mut handlebars: handlebars::Handlebars<'static> = handlebars::Handlebars::new();
    handlebars.set_strict_mode(true);
    let handlebars = Arc::new(handlebars);

    context.insert(
        "DOD_ROOT".to_string(),
        utils::file_fs::path_to_string(&dotdeploy_config.config_root)?,
    );
    context.insert(
        "DOD_MODULES_ROOT".to_string(),
        utils::file_fs::path_to_string(&dotdeploy_config.modules_root)?,
    );
    context.insert(
        "DOD_HOSTS_ROOT".to_string(),
        utils::file_fs::path_to_string(&dotdeploy_config.hosts_root)?,
    );
    context.insert(
        "DOD_HOSTNAME".to_string(),
        dotdeploy_config.hostname.to_string(),
    );
    context.insert(
        "DOD_DISTRO".to_string(),
        dotdeploy_config.distribution.to_string(),
    );

    let mut messages: (
        std::collections::BTreeMap<String, Vec<String>>,
        std::collections::BTreeMap<String, Vec<String>>,
    ) = (
        std::collections::BTreeMap::new(),
        std::collections::BTreeMap::new(),
    );

    let mut generators: std::collections::BTreeMap<std::path::PathBuf, crate::modules::generate::Generate> =
        std::collections::BTreeMap::new();

    // Initialize stores
    let stores = Arc::new(Stores::init().await.context("Failed to initialize stores")?);

    match &cli.command {
        cli::Commands::Deploy { modules } => match modules {
            None => {
                let mut module_queue = modules::queue::ModuleQueue {
                    modules: std::collections::BTreeSet::new(),
                    context,
                };
                // Try to add host module
                let host_module = vec![["hosts/", &dotdeploy_config.hostname].join("").to_string()];
                module_queue.add_modules(&host_module, &dotdeploy_config, true)?;

                trace!("Context values: {:#?}", &module_queue.context);

                // Add modules to stores
                for module in module_queue.modules.iter() {
                    let m = crate::store::modules::StoreModule {
                        name: module.name.clone(),
                        location: utils::file_fs::path_to_string(&module.location)?,
                        user: Some(std::env::var("USER")?),
                        reason: module.reason.clone(),
                        depends: module.config.depends.clone().map(|deps| deps.join(", ")),
                        date: chrono::offset::Local::now(),
                    };
                    // User store
                    stores
                        .user_store
                        .add_module(m.clone())
                        .await
                        .map_err(|e| e.into_anyhow())?;
                    // System store
                    if let Some(ref sys_store) = stores.system_store {
                        sys_store.add_module(m).await.map_err(|e| e.into_anyhow())?;
                    }
                }

                let phases = phases::assign_module_config(
                    module_queue.modules,
                    serde_json::to_value(&module_queue.context)?,
                    &stores,
                    &mut messages,
                    &mut generators,
                    &handlebars,
                )
                .await?;

                crate::deploy::deploy(
                    phases,
                    Arc::clone(&stores),
                    serde_json::to_value(&module_queue.context)?,
                    Arc::clone(&handlebars),
                    &dotdeploy_config,
                )
                .await?;

                // Generate files
                crate::modules::generate::generate_files(
                    Arc::clone(&stores),
                    generators,
                    serde_json::to_value(&module_queue.context)?,
                    handlebars,
                )
                .await?;

                // Close pools and save their location
                let user_store_path = stores.user_store.path.clone();
                let mut sys_store_path = std::path::PathBuf::new();

                stores.user_store.close().await.map_err(|e| e.into_anyhow())?;
                if let Some(sys_store) = &stores.system_store {
                    sys_store_path.push(sys_store.path.clone());
                    sys_store.close().await.map_err(|e| e.into_anyhow())?;
                }

                // Drop seems to be the way to make sure the connections get closed
                drop(stores);

                // Wait until SQLite cleans up the WAL and SHM files
                store::db::close_connection(&user_store_path)?;
                if !sys_store_path.as_os_str().is_empty() {
                    store::db::close_connection(&sys_store_path)?;
                }

                // Display messages
                for (module, msgs) in messages.0.into_iter() {
                    info!("Message for {}", module);
                    for m in msgs.into_iter() {
                        println!("{}", m)
                    }
                }

                Ok(true)
            }
            Some(_modules) => {
                warn!("Not implemented yet");
                Ok(true)
            }
        },
        cli::Commands::Remove { modules } => match modules {
            None => {
                warn!("Not implemented yet");
                Ok(true)
            }
            Some(modules) => {

                // let mut modules = vec![["hosts/", &dotdeploy_config.hostname.unwrap()].join("")];
                let module_configs = std::collections::BTreeSet::new();
                let mut files: Vec<crate::store::files::StoreFile> = vec![];
                // Try to add host module
                // let host_module = ["hosts/", &dotdeploy_config.hostname].join("");
                let mut module_queue = modules::queue::ModuleQueue {
                    modules: std::collections::BTreeSet::new(),
                    context,
                };

                module_queue.add_modules(modules, &dotdeploy_config, true)?;

                for module in modules.iter() {
                    let user_files = stores
                        .user_store
                        .get_all_files(&module)
                        .await
                        .map_err(|e| e.into_anyhow())
                        .with_context(|| {
                            format!(
                                "Failed to get files for module {:?} from user store",
                                &module
                            )
                        })?;
                    for f in user_files.into_iter() {
                        files.push(f);
                    }

                    if let Some(sys_store) = &stores.system_store {
                        let sys_files = sys_store
                            .get_all_files(&module)
                            .await
                            .map_err(|e| e.into_anyhow())
                            .with_context(|| {
                                format!(
                                    "Failed to get files for module {:?} from system store",
                                    &module
                                )
                            })?;
                        for f in sys_files.into_iter() {
                            files.push(f);
                        }
                    };
                }

                let phases = phases::assign_module_config(
                    module_configs,
                    serde_json::to_value(&module_queue.context)?,
                    &stores,
                    &mut messages,
                    &mut generators,
                    &handlebars,
                )
                .await?;

                crate::remove::remove(phases, Arc::clone(&stores), files, &dotdeploy_config)
                    .await?;

                // Remove modules from the stores
                for module in modules.iter() {
                    stores
                        .user_store
                        .remove_module(module)
                        .await
                        .map_err(|e| e.into_anyhow())?;
                    if let Some(sys_store) = &stores.system_store {
                        sys_store
                            .remove_module(module)
                            .await
                            .map_err(|e| e.into_anyhow())?;
                    }
                }

                // Generate files
                crate::modules::generate::generate_files(
                    Arc::clone(&stores),
                    generators,
                    serde_json::to_value(&module_queue.context)?,
                    handlebars,
                )
                .await?;

                // Close pools and save their location
                let user_store_path = stores.user_store.path.clone();
                let mut sys_store_path = std::path::PathBuf::new();

                stores.user_store.close().await.map_err(|e| e.into_anyhow())?;
                if let Some(sys_store) = &stores.system_store {
                    sys_store_path.push(sys_store.path.clone());
                    sys_store.close().await.map_err(|e| e.into_anyhow())?;
                }

                // Drop seems to be the way to make sure the connections get closed
                drop(stores);

                // Wait until SQLite cleans up the WAL and SHM files
                store::db::close_connection(&user_store_path)?;
                if !sys_store_path.as_os_str().is_empty() {
                    store::db::close_connection(&sys_store_path)?;
                }

                // Display messages
                for (module, msgs) in messages.1.into_iter() {
                    info!("Message for {}", module);
                    for m in msgs.into_iter() {
                        println!("{}", m)
                    }
                }

                Ok(true)
            }
        },
    }
}
