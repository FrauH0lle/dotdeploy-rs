use crate::cmds::common;
use crate::store::Store;
use crate::store::sqlite_files::StoreFile;
use crate::store::sqlite_modules::StoreModuleBuilder;
use crate::utils::common::os_str_to_bytes;
use crate::utils::{FileUtils, file_fs};
use crate::{
    config::DotdeployConfig, modules::queue::ModulesQueueBuilder, store::Stores,
    utils::sudo::PrivilegeManager,
};
use color_eyre::eyre::{OptionExt, WrapErr, eyre};
use color_eyre::{Report, Result};
use handlebars::Handlebars;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::sync::Arc;
use tokio::task::JoinSet;
use toml::Value;
use tracing::{debug, info, warn};

/// Executes the full deployment pipeline for specified modules
///
/// Orchestrates the deployment process including:
/// - Module dependency resolution
/// - Context collection and template processing  
/// - Phase-based deployment (setup/config/update)
/// - Package management
/// - File generation and message handling
///
/// # Arguments
/// * `modules` - Module names to deploy
/// * `config` - Shared application configuration
/// * `stores` - Database stores for deployment tracking
/// * `context` - Template context variables
/// * `handlebars` - Template engine registry
/// * `pm` - Privilege manager for elevated operations
///
/// # Errors
/// Returns error if any deployment phase fails or invalid configuration is detected
pub(crate) async fn deploy(
    modules: Vec<String>,
    config: Arc<DotdeployConfig>,
    stores: Arc<Stores>,
    mut context: HashMap<String, Value>,
    handlebars: Handlebars<'static>,
    pm: Arc<PrivilegeManager>,
) -> Result<bool> {
    let mut mod_queue = ModulesQueueBuilder::new()
        .with_modules(modules)
        .build(&config)?;

    let module_names = mod_queue.collect_module_names(&mut context);
    mod_queue
        .collect_context(&mut context)
        .wrap_err("Failed to collect context")?;
    mod_queue.finalize(&context, &handlebars)?;

    // Ensure modules are available in the store
    let mut set = JoinSet::new();
    for module in mod_queue.modules.iter() {
        let name = module.name.clone();
        let location = module.location.to_string_lossy().to_string();
        let location_u8 = os_str_to_bytes(&module.location);
        let user = Some(whoami::username());
        let reason = module.reason.clone();
        let depends = module.depends_on.clone();
        let date = chrono::offset::Utc::now();
        let stores = Arc::clone(&stores);
        set.spawn(async move {
            stores
                .add_module(
                    &StoreModuleBuilder::default()
                        .with_name(&name)
                        .with_location(location)
                        .with_location_u8(location_u8)
                        .with_user(user)
                        .with_reason(reason)
                        .with_depends(depends)
                        .with_date(date)
                        .build()?,
                )
                .await
                .wrap_err(format!("Failed to add module '{}' to store", name))
        });
    }
    crate::errors::join_errors(set.join_all().await)?;

    let (
        mut setup_phase,
        mut config_phase,
        mut update_phase,
        mut remove_phase,
        mut packages,
        file_generators,
        module_messages,
    ) = mod_queue.process(Arc::clone(&config)).await?;

    // Sanitize packages & and check install condition
    packages.retain(|p| !p.package.is_empty());
    if !packages.is_empty() && config.install_pkg_cmd.is_none() {
        dbg!(&packages);
        return Err(eyre!(
            "Found packages to install, but `install_pkg_cmd` is not defined"
        ));
    }

    let deployed_modules = stores
        .get_all_modules()
        .await?
        .into_iter()
        .map(|m| m.name)
        .collect::<HashSet<_>>();

    let deployed_files = collect_deployed_files(deployed_modules, Arc::clone(&stores)).await?;
    let modified_files = validate_deployed_files(
        deployed_files,
        Arc::clone(&config),
        Arc::clone(&pm),
        Arc::clone(&stores),
    )
    .await?;

    // Notify that files were modified outside of dotdeploy and ask for confirmation.
    if !modified_files.is_empty() {
        warn!(
            "The following files were modified outside of dotdeploy:{}{}",
            format!("\n  - {}", modified_files.join("\n  - ")),
            "\n!! Changes will be overwritten !! \n"
        );
        if !(config.force
            || crate::utils::common::ask_boolean(&format!(
                "{}\n{}",
                "Do you want to continue [y/N]?",
                "(You can skip this prompt with the CLI argument '-f true' or '--force=true')",
            )))
        {
            warn!("Aborted by user");
            return Ok(false);
        }
    }

    // Wrap handlebars and context in an Arc as they will be shared across threads
    let hb = Arc::new(handlebars);
    let context = Arc::new(context);

    debug!("Running SETUP phase");
    setup_phase.exec_pre_tasks(&pm, &config).await?;
    setup_phase
        .deploy_files(
            Arc::clone(&pm),
            Arc::clone(&stores),
            Arc::clone(&context),
            Arc::clone(&hb),
        )
        .await?;
    setup_phase.exec_post_tasks(&pm, &config).await?;
    debug!("SETUP phase complete");

    // Install packages
    if config.skip_pkg_install {
        info!("Skipping package installation as requested");
    } else if !packages.is_empty() {
        info!("Installing packages");

        // Verify installed packages
        let mut obsolete = vec![];
        let pkg_modules = packages
            .iter()
            .map(|x| &x.module_name)
            .collect::<HashSet<_>>();

        // For each module, get all registered packages
        // REVIEW 2025-03-28: Can be aysnc
        for pmod in pkg_modules {
            let store_pkgs: HashSet<String> =
                HashSet::from_iter(stores.get_all_module_packages(&pmod).await?.into_iter());
            let requested_pkgs = HashSet::from_iter(
                packages
                    .iter()
                    .filter(|p| p.module_name == *pmod)
                    .map(|p| p.package.clone()),
            );
            // The packages which are in store but not in the config anymore -> Should be removed
            let diff = store_pkgs
                .difference(&requested_pkgs)
                .collect::<HashSet<_>>();
            let other_module_pkgs = stores.get_all_other_module_packages(&pmod).await?;
            // Drop packages for module
            for p in diff {
                stores.remove_package(pmod, p).await?;
                if !other_module_pkgs.contains(pmod) {
                    obsolete.push(p.to_string());
                }
            }
        }

        // Remove obsolete packages

        // REVIEW 2025-03-21: Remove empty string
        obsolete.retain(|p| !p.is_empty());
        if !obsolete.is_empty() {
            let obsolete = obsolete.into_iter().map(OsString::from).collect::<Vec<_>>();
            common::exec_package_cmd(config.remove_pkg_cmd.as_ref().unwrap(), &obsolete, &pm)
                .await?;
        }

        // Add packages to store

        // REVIEW 2025-03-21: Remove empty string
        let packages = packages
            .into_iter()
            .filter(|p| !p.package.is_empty())
            .collect::<Vec<_>>();

        // REVIEW 2025-03-28: Can be aysnc
        for p in packages.iter() {
            stores.add_package(&p.module_name, &p.package).await?
        }

        let packages = packages
            .into_iter()
            .map(|p| OsString::from(p.package))
            .collect::<Vec<_>>();

        if !packages.is_empty() {
            common::exec_package_cmd(
                config
                    .install_pkg_cmd
                    .as_ref()
                    .ok_or_eyre("Missing package install command in config")?,
                &packages,
                &pm,
            )
            .await
            .wrap_err("Failed to install required packages")?;
        }

        info!("Package installation complete");
    }

    debug!("Running CONFIG phase");
    config_phase.exec_pre_tasks(&pm, &config).await?;
    config_phase
        .deploy_files(
            Arc::clone(&pm),
            Arc::clone(&stores),
            Arc::clone(&context),
            Arc::clone(&hb),
        )
        .await?;
    config_phase.exec_post_tasks(&pm, &config).await?;
    debug!("CONFIG phase complete");

    // Generate files
    debug!("Generating files");
    // REVIEW 2025-03-28: Can be aysnc
    for file in file_generators {
        file.generate_file(&stores, &context, &hb, &config, Arc::clone(&pm))
            .await?;
    }
    debug!("Generating files complete");

    // Display messages and update cache
    debug!("Displaying messages");

    // Drop old messages
    // REVIEW 2025-03-28: Make a loop
    for module in module_names.iter() {
        stores
            .user_store
            .remove_all_cached_messages(module.as_str(), "update")
            .await?;
        stores
            .user_store
            .remove_all_cached_messages(module.as_str(), "remove")
            .await?;
    }

    // Add new messages
    for msg in module_messages.into_iter() {
        match msg.on_command.as_deref() {
            Some("deploy") => info!("Message for {}:\n{}", msg.module_name, msg.message),
            Some("update") => stores.user_store.cache_message("update", msg).await?,
            Some("remove") => stores.user_store.cache_message("remove", msg).await?,
            _ => unreachable!(),
        }
    }

    // Cache update and remove phase
    // REVIEW 2025-03-28: Make a loop
    if let Some(mut cached_update_tasks) = stores.user_store.get_cached_commands("update").await? {
        cached_update_tasks
            .tasks
            .retain(|t| !module_names.contains(&t.module_name));
        update_phase.tasks.append(&mut cached_update_tasks.tasks);
    }
    if let Some(mut cached_remove_tasks) = stores.user_store.get_cached_commands("remove").await? {
        cached_remove_tasks
            .tasks
            .retain(|t| !module_names.contains(&t.module_name));
        remove_phase.tasks.append(&mut cached_remove_tasks.tasks);
    }

    stores
        .user_store
        .cache_command("update", update_phase)
        .await?;
    stores
        .user_store
        .cache_command("remove", remove_phase)
        .await?;

    debug!("Deploy command complete");
    Ok(true)
}

async fn collect_deployed_files<I>(
    deployed_modules: I,
    stores: Arc<Stores>,
) -> Result<Vec<StoreFile>>
where
    I: IntoIterator<Item = String>,
{
    let mut set = JoinSet::new();

    for name in deployed_modules {
        set.spawn({
            let stores = Arc::clone(&stores);
            let name = name;
            async move {
                let files = stores.get_all_files(name).await?;
                Ok::<_, Report>(files)
            }
        });
    }

    let deployed_files = crate::errors::join_errors(set.join_all().await)?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

    Ok(deployed_files)
}

async fn validate_deployed_files<I>(
    deployed_files: I,
    config: Arc<DotdeployConfig>,
    pm: Arc<PrivilegeManager>,
    stores: Arc<Stores>,
) -> Result<Vec<String>>
where
    I: IntoIterator<Item = StoreFile>,
{
    let mut set = JoinSet::new();

    for file in deployed_files {
        set.spawn({
            let file_utils = FileUtils::new(Arc::clone(&pm));
            let config = Arc::clone(&config);
            let stores = Arc::clone(&stores);
            async move {
                // Check if source still exists
                if let Some(ref source) = file.source {
                    if !file_utils.check_file_exists(source).await? {
                        info!(
                            "Source file {} does not exist anymore, removing deployed target {}",
                            &source, &file.target
                        );
                        file_utils.delete_file(&file.target).await?;

                        // Restore backup, if any
                        if stores.check_backup_exists(&file.target).await? {
                            stores.restore_backup(&file.target, &file.target).await?;
                            // Remove backup
                            stores.remove_backup(&file.target).await?;
                        }

                        // Remove file from store
                        stores.remove_file(&file.target).await?;

                        // Delete potentially empty directories
                        file_utils
                            .delete_parents(&file.target, config.noconfirm)
                            .await?
                    }
                }

                // Always remove dynamically created files
                if file.operation.as_str() == "create" || file.operation.as_str() == "generate" {
                    debug!(
                        "Removing deployed target for dynamically created file {}",
                        &file.target
                    );
                    file_utils.delete_file(&file.target).await?;

                    // Restore backup, if any
                    if stores.check_backup_exists(&file.target).await? {
                        stores.restore_backup(&file.target, &file.target).await?;
                        // Remove backup
                        stores.remove_backup(&file.target).await?;
                    }

                    // Remove file from store
                    stores.remove_file(&file.target).await?;

                    // Delete potentially empty directories
                    file_utils
                        .delete_parents(&file.target, config.noconfirm)
                        .await?
                }

                let mut modified_files = vec![];
                // Check if target was modified
                if file_utils.check_file_exists(&file.target).await? {
                    let file_checksum = file_utils.calculate_sha256_checksum(&file.target).await?;
                    let store_checksum = stores.get_target_checksum(&file.target).await?;
                    if let Some(store_checksum) = store_checksum.target_checksum {
                        if file_checksum != store_checksum {
                            modified_files.push(file.target);
                        }
                    }
                }

                Ok::<_, Report>(modified_files)
            }
        });
    }

    let modified_files = crate::errors::join_errors(set.join_all().await)?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

    Ok(modified_files)
}
