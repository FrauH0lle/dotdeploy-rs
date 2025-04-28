use crate::cli::SyncComponent;
use crate::cmds::common;
use crate::config::DotdeployConfig;
use crate::errors;
use crate::modules::DeployPhase;
use crate::modules::queue::{ModulesQueue, ModulesQueueBuilder};
use crate::phases::DeployPhaseTasks;
use crate::store::Store;
use crate::store::sqlite::SQLiteStore;
use crate::store::sqlite_files::StoreFile;
use crate::store::sqlite_modules::StoreModuleBuilder;
use crate::utils::FileUtils;
use crate::utils::common::os_str_to_bytes;
use crate::utils::sudo::PrivilegeManager;
use color_eyre::eyre::{OptionExt, WrapErr, eyre};
use color_eyre::{Report, Result};
use handlebars::Handlebars;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::sync::Arc;
use tokio::task::JoinSet;
use toml::Value;
use tracing::{debug, error, info, warn};

pub(crate) struct SyncCtx {
    pub(crate) config: Arc<DotdeployConfig>,
    pub(crate) components: Vec<SyncComponent>,
    pub(crate) store: Arc<SQLiteStore>,
    pub(crate) context: HashMap<String, Value>,
    pub(crate) handlebars: Handlebars<'static>,
    pub(crate) pm: Arc<PrivilegeManager>,
}

pub(crate) enum SyncOp {
    Deploy,
    Sync,
}

/// Synchronize module components
///
/// Orchestrates the synchronization of modules through these key phases:
/// 1. Module dependency resolution and context preparation
/// 2. Phase-based deployment (setup, config)
/// 3. Package installation/removal
/// 4. File generation and message handling
/// 5. Cleanup of obsolete modules and files
///
/// The process is influenced by the chosen components.
///
/// # Arguments
/// * `modules` - Names of modules to deploy
/// * `config` - Shared application configuration with deployment settings
/// * `components` - Which deployment components to synchronize (files, tasks, packages)
/// * `store` - Database store for tracking deployed artifacts
/// * `context` - Template variables for handlebars processing  
/// * `handlebars` - Handlebars template registry instance
/// * `pm` - Privilege manager handling sudo/root operations
///
/// # Errors
/// Returns error if:
/// - Module dependency resolution fails
/// - File deployment fails (permission issues, invalid paths)
/// - Package management commands are undefined in config when needed
/// - Task processing encounters invalid definitions
/// - Template rendering errors occur
/// - Store operations fail (database errors)
pub(crate) async fn sync(
    modules: Vec<String>,
    ctx: SyncCtx,
    op: SyncOp,
    show_msgs: bool,
) -> Result<bool> {
    // Destructure ctx
    let SyncCtx {
        config,
        components,
        store,
        mut context,
        handlebars,
        pm,
    } = ctx;

    let sync_files = components.iter().any(|c| c.is_files() || c.is_all());
    let sync_tasks = components.iter().any(|c| c.is_tasks() || c.is_all());
    let sync_packages = components.iter().any(|c| c.is_packages() || c.is_all());
    let show_messages = show_msgs;

    let mut mod_queue = ModulesQueueBuilder::new()
        .with_modules(modules)
        .build(&config)?;

    // Add queued modules to context
    let module_names = mod_queue.collect_module_names(&mut context);

    // Make queued modules available as the env var DOD_MODULES="mod1,mod2,mod3"
    unsafe { std::env::set_var("DOD_MODULES", module_names.join(",")) }

    mod_queue
        .collect_context(&mut context)
        .wrap_err("Failed to collect context")?;
    mod_queue.finalize(&mut context, &handlebars)?;

    match op {
        // Ensure modules are added to the store
        SyncOp::Deploy => ensure_modules(&mod_queue, Arc::clone(&store)).await?,
        // Ensure we are only syncing modules which are already deployed
        SyncOp::Sync => {
            let dpl_mods = store
                .get_all_modules()
                .await?
                .into_iter()
                .map(|m| m.name)
                .collect::<HashSet<_>>();
            let unknown_modules = module_names
                .iter()
                .filter(|&m| !dpl_mods.contains(m))
                .map(|m| m.as_str())
                .collect::<Vec<_>>();
            if !unknown_modules.is_empty() {
                error!(
                    "The following modules are not yet deployed:{}",
                    format!("\n  - {}", unknown_modules.join("\n  - "))
                );
                return Ok(false);
            }
        }
    }

    // Check for automatically installed modules that are no longer required as dependencies by any
    // other modules. These orphaned modules can be safely removed since they were only added to
    // satisfy previous dependencies.
    remove_obsolete_modules(
        Arc::clone(&store),
        Arc::clone(&config),
        &context,
        &handlebars,
        Arc::clone(&pm),
    )
    .await?;

    // Process queue into phases
    let (
        mut setup_phase_files,
        mut config_phase_files,
        mut task_container,
        mut packages,
        module_messages,
    ) = mod_queue
        .process(Arc::clone(&config), Arc::clone(&store), Arc::clone(&pm))
        .await?;

    // Check if tasks changed and remove obsolete ones
    if sync_tasks {
        task_container = remove_obsolete_tasks(
            module_names,
            Arc::clone(&store),
            task_container,
            Arc::clone(&config),
            Arc::clone(&pm),
        )
        .await?;
    }

    if sync_packages {
        // Perform checks for package installation and removal
        //
        // 1. Partition packages into dummy packages and real packages
        // 2. Check if the modules of the dummy packages have already packages in the store -> Requires
        //    `remove_pkg_command` to be set
        // 3. Check if real packages is not empty -> Requires `install_pkg_command` to be set
        let (mut dummy_packages, mut pkgs): (Vec<_>, Vec<_>) =
            packages.into_iter().partition(|p| p.package.is_empty());

        let mut set = JoinSet::new();
        for dp in dummy_packages.iter() {
            let m = dp.module_name.clone();
            let store = Arc::clone(&store);
            set.spawn(async move { store.get_all_module_packages(m).await });
        }
        let installed_pkgs = errors::join_errors(set.join_all().await)?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();

        if !installed_pkgs.is_empty() && config.remove_pkg_cmd.is_none() {
            return Err(eyre!(
                "Found packages in store which might need to get removed but `remove_pkg_cmd` is not defined"
            ));
        }

        if !pkgs.is_empty() && config.install_pkg_cmd.is_none() {
            return Err(eyre!(
                "Found packages to install but `install_pkg_cmd` is not defined"
            ));
        }

        // Merge dummy and packages again for further processing
        pkgs.append(&mut dummy_packages);
        packages = pkgs;
    }

    if sync_files {
        // Validate deployed files
        let deployed_modules = store
            .get_all_modules()
            .await?
            .into_iter()
            .map(|m| m.name)
            .collect::<HashSet<_>>();

        let deployed_files = collect_deployed_files(deployed_modules, Arc::clone(&store)).await?;
        let modified_files = validate_deployed_files(
            deployed_files,
            Arc::clone(&config),
            Arc::clone(&pm),
            Arc::clone(&store),
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
                    "(You can skip this prompt with the CLI argument '-f/--force true')",
                )))
            {
                warn!("Aborted by user");
                return Ok(false);
            }
        }
    }

    // Wrap handlebars and context in an Arc as they will be shared across threads
    let hb = Arc::new(handlebars);
    let context = Arc::new(context);

    // Run deployment
    debug!("Running SETUP phase");
    if sync_tasks {
        task_container
            .exec_pre_tasks(&pm, &config, DeployPhase::Setup)
            .await?;
    }
    if sync_files {
        setup_phase_files
            .deploy_files(
                Arc::clone(&pm),
                Arc::clone(&store),
                Arc::clone(&context),
                Arc::clone(&hb),
            )
            .await?;
    }
    if sync_tasks {
        task_container
            .exec_post_tasks(&pm, &config, DeployPhase::Setup)
            .await?;
    }
    debug!("SETUP phase complete");

    // Install packages
    if sync_packages {
        if !packages.is_empty() {
            info!("Installing packages");

            // Verify installed packages and collect obsolete packages for removal
            // Collect modules from requested packages
            let pkg_modules = packages
                .iter()
                .map(|x| x.module_name.clone())
                .collect::<HashSet<_>>();

            // For each module, get all registered packages from store
            let packages = Arc::new(packages);

            let mut set = JoinSet::new();
            for module_name in pkg_modules.into_iter() {
                set.spawn({
                    let packages = Arc::clone(&packages);
                    let store = Arc::clone(&store);
                    async move {
                        let mut obsolete = vec![];

                        let store_pkgs: HashSet<String> = HashSet::from_iter(
                            store
                                .get_all_module_packages(&module_name)
                                .await?
                                .into_iter(),
                        );

                        let requested_pkgs = HashSet::from_iter(
                            packages
                                .iter()
                                .filter(|p| p.module_name == *module_name)
                                .map(|p| p.package.clone()),
                        );

                        // The packages which are in store but not in the config anymore -> Should be removed
                        let diff = store_pkgs
                            .difference(&requested_pkgs)
                            .collect::<HashSet<_>>();
                        let other_module_pkgs =
                            store.get_all_other_module_packages(&module_name).await?;

                        // Drop packages for module
                        for p in diff {
                            store.remove_package(module_name.as_str(), p).await?;
                            if !other_module_pkgs.contains(&module_name) {
                                obsolete.push(p.to_string());
                            }
                        }
                        Ok(obsolete)
                    }
                });
            }
            // Collect obsolete packages and remove empty string/dummy ones
            let obsolete = errors::join_errors(set.join_all().await)?
                .into_iter()
                .flatten()
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>();

            // Remove obsolete packages
            if !obsolete.is_empty() {
                let obsolete = obsolete.into_iter().map(OsString::from).collect::<Vec<_>>();
                common::exec_package_cmd(
                    config
                        .remove_pkg_cmd
                        .as_ref()
                        .ok_or_eyre("`remove_pkg_cmd` not defined in config")?,
                    &obsolete,
                    &pm,
                )
                .await?;
            }

            // Add packages to store
            let packages = Arc::try_unwrap(packages)
                .map_err(|e| eyre!("Failed to unwrap Arc {:?}", e))?
                .into_iter()
                .filter(|p| !p.package.is_empty())
                .collect::<Vec<_>>();

            let mut set = JoinSet::new();
            for p in packages.into_iter() {
                set.spawn({
                    let store = Arc::clone(&store);
                    async move {
                        store.add_package(&p.module_name, &p.package).await?;
                        Ok::<_, Report>(p)
                    }
                });
            }
            let packages = crate::errors::join_errors(set.join_all().await)?;

            // Transform package names into OsString and collect them
            let packages = packages
                .into_iter()
                .map(|p| OsString::from(p.package))
                .collect::<Vec<_>>();

            // Finally install the packages
            if !packages.is_empty() {
                common::exec_package_cmd(
                    config
                        .install_pkg_cmd
                        .as_ref()
                        .ok_or_eyre("`install_pkg_cmd` not defined in config")?,
                    &packages,
                    &pm,
                )
                .await
                .wrap_err("Failed to install required packages")?;
            }

            info!("Package installation complete");
        }
    }

    debug!("Running CONFIG phase");
    if sync_tasks {
        task_container
            .exec_pre_tasks(&pm, &config, DeployPhase::Config)
            .await?;
    }
    if sync_files {
        config_phase_files
            .deploy_files(
                Arc::clone(&pm),
                Arc::clone(&store),
                Arc::clone(&context),
                Arc::clone(&hb),
            )
            .await?;
    }
    if sync_tasks {
        task_container
            .exec_post_tasks(&pm, &config, DeployPhase::Config)
            .await?;
    }
    debug!("CONFIG phase complete");

    // Generate files
    debug!("Generating files");

    // REVIEW 2025-04-28: This should be done in a better way.
    let hb = Arc::try_unwrap(hb).map_err(|e| eyre!("Failed to unwrap Arc {:?}", e))?;
    let mut context =
        Arc::try_unwrap(context).map_err(|e| eyre!("Failed to unwrap Arc {:?}", e))?;

    let mut file_gen_queue = ModulesQueueBuilder::new()
        .with_modules(
            store
                .get_all_modules()
                .await?
                .into_iter()
                .filter(|m| m.name.as_str() != "__dotdeploy_generated")
                .map(|m| m.name)
                .collect::<Vec<_>>(),
        )
        .build(&config)?;

    let module_names = file_gen_queue.collect_module_names(&mut context);

    // Make queued modules available as the env var DOD_MODULES="mod1,mod2,mod3"
    unsafe { std::env::set_var("DOD_MODULES", module_names.join(",")) }

    file_gen_queue
        .collect_context(&mut context)
        .wrap_err("Failed to collect context")?;
    file_gen_queue.finalize(&mut context, &hb)?;

    let file_generators = file_gen_queue.get_file_generators().await?;

    let mut set = JoinSet::new();
    // REVIEW 2025-04-28: This should be done in a better way.
    let hb = Arc::new(hb);
    let context = Arc::new(context);
    for file in file_generators {
        set.spawn({
            let store = Arc::clone(&store);
            let context = Arc::clone(&context);
            let hb = Arc::clone(&hb);
            let config = Arc::clone(&config);
            let pm = Arc::clone(&pm);
            async move { file.generate_file(&store, &context, &hb, &config, pm).await }
        });
    }
    errors::join_errors(set.join_all().await)?;
    debug!("Generating files complete");

    // Display messages and update cache
    debug!("Displaying messages");

    // Drop old messages
    let mut set = JoinSet::new();
    for module in module_names.into_iter() {
        set.spawn({
            let store = Arc::clone(&store);
            async move {
                store
                    .remove_all_cached_messages(module.as_str(), "deploy")
                    .await?;
                store
                    .remove_all_cached_messages(module.as_str(), "update")
                    .await?;
                store
                    .remove_all_cached_messages(module.as_str(), "remove")
                    .await?;
                Ok(module)
            }
        });
    }

    errors::join_errors(set.join_all().await)?;

    // Add new messages
    for msg in module_messages.into_iter() {
        match msg.on_command.as_deref() {
            Some("deploy") => {
                if show_messages {
                    info!("Message for {}:\n{}", msg.module_name, msg.message)
                };
                store.cache_message("deploy", msg).await?
            }
            Some("update") => store.cache_message("update", msg).await?,
            Some("remove") => store.cache_message("remove", msg).await?,
            _ => unreachable!(),
        }
    }

    // Cache tasks
    if sync_tasks {
        let mut set = JoinSet::new();
        for task in task_container.tasks.into_iter() {
            let store = Arc::clone(&store);
            set.spawn(async move { store.add_task(task).await });
        }
        errors::join_errors(set.join_all().await)?;
    }

    match op {
        SyncOp::Deploy => debug!("Deploy command complete"),
        SyncOp::Sync => debug!("Sync command complete"),
    }

    Ok(true)
}

async fn collect_deployed_files<I>(
    deployed_modules: I,
    store: Arc<SQLiteStore>,
) -> Result<Vec<StoreFile>>
where
    I: IntoIterator<Item = String>,
{
    let mut set = JoinSet::new();

    for name in deployed_modules {
        set.spawn({
            let store = Arc::clone(&store);
            async move {
                let files = store.get_all_files(name).await?;
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
    store: Arc<SQLiteStore>,
) -> Result<Vec<String>>
where
    I: IntoIterator<Item = StoreFile>,
{
    let mut set = JoinSet::new();
    let guard = Arc::new(tokio::sync::Mutex::new(()));

    for file in deployed_files {
        set.spawn({
            let file_utils = FileUtils::new(Arc::clone(&pm));
            let config = Arc::clone(&config);
            let store = Arc::clone(&store);
            let guard = Arc::clone(&guard);
            async move {
                // Check if source still exists
                if let Some(ref source) = file.source {
                    if !file_utils.check_path_exists(source).await? {
                        info!(
                            "Source file {} does not exist anymore, removing deployed target {}",
                            &source, &file.target
                        );
                        file_utils.delete_file(&file.target).await?;

                        // Restore backup, if any
                        if store.check_backup_exists(&file.target).await? {
                            store.restore_backup(&file.target, &file.target).await?;
                            // Remove backup
                            store.remove_backup(&file.target).await?;
                        }

                        // Remove file from store
                        store.remove_file(&file.target).await?;

                        // Delete potentially empty directories
                        let guard = Arc::clone(&guard);
                        file_utils
                            .delete_parents(&file.target, config.noconfirm, Some(guard))
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
                    if store.check_backup_exists(&file.target).await? {
                        store.restore_backup(&file.target, &file.target).await?;
                        // Remove backup
                        store.remove_backup(&file.target).await?;
                    }

                    // Remove file from store
                    store.remove_file(&file.target).await?;

                    // Delete potentially empty directories
                    file_utils
                        .delete_parents(&file.target, config.noconfirm, Some(guard))
                        .await?
                }

                let mut modified_files = vec![];
                // Check if target was modified
                if file_utils.check_path_exists(&file.target).await? {
                    let file_checksum = file_utils.calculate_sha256_checksum(&file.target).await?;
                    let store_checksum = store.get_target_checksum(&file.target).await?;
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

async fn ensure_modules(module_queue: &ModulesQueue, store: Arc<SQLiteStore>) -> Result<()> {
    let mut set = JoinSet::new();
    for module in module_queue.modules.iter() {
        let name = module.name.clone();
        let location = module.location.to_string_lossy().to_string();
        let location_u8 = os_str_to_bytes(&module.location);
        let user = Some(whoami::username());
        let reason = module.reason.clone();
        let depends = module.depends_on.clone();
        let date = chrono::offset::Utc::now();
        let store = Arc::clone(&store);

        // Get old module data from store
        if let Some(old_module) = store.get_module(&name).await? {
            if location_u8 != old_module.location_u8 {
                warn!(
                    "{}: module's location changed from {} to {}",
                    &name, &old_module.location, &location
                )
            }

            if reason != old_module.reason {
                warn!(
                    "{}: module's installation reason changed from {} to {}",
                    &name, &old_module.reason, &reason
                )
            }
        }

        set.spawn(async move {
            store
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
    Ok(())
}

async fn remove_obsolete_modules(
    store: Arc<SQLiteStore>,
    config: Arc<DotdeployConfig>,
    context: &HashMap<String, Value>,
    handlebars: &Handlebars<'static>,
    pm: Arc<PrivilegeManager>,
) -> Result<()> {
    let mut obsolete_modules = HashSet::new();

    // Get all modules from store that were automatically added
    let auto_modules = store
        .get_all_modules()
        .await?
        .into_iter()
        .filter(|m| m.reason == "automatic" && m.name != "__dotdeploy_generated")
        .map(|m| m.name)
        .collect::<HashSet<_>>();

    // Get all current module dependencies
    let mut all_dependencies = HashSet::new();
    for module in store.get_all_modules().await? {
        if let Some(deps) = module.depends {
            all_dependencies.extend(deps);
        }
    }

    // Find automatic modules that aren't dependencies anymore
    for module in auto_modules {
        if !all_dependencies.contains(&module) {
            obsolete_modules.insert(module);
        }
    }
    let obsolete_modules = Vec::from_iter(obsolete_modules.into_iter());
    if !obsolete_modules.is_empty() {
        warn!(
            "The following automatically installed modules are no longer needed as dependencies:{}{}",
            format!("\n  - {}", obsolete_modules.join("\n  - ")),
            "\n!! These modules will be removed !! \n"
        );

        if !(config.force
            || config.noconfirm
            || crate::utils::common::ask_boolean(&format!(
                "{}\n{}",
                "Do you want to remove these modules? [y/N]?",
                "(You can skip this prompt with the CLI argument '-y/--noconfirm true' or '-f/--force true')"
            )))
        {
            warn!("Keeping obsolete modules as requested by user");
        } else {
            // Remove the obsolete modules
            crate::cmds::remove::remove(
                obsolete_modules,
                Arc::clone(&config),
                Arc::clone(&store),
                Clone::clone(&context),
                Clone::clone(&handlebars),
                Arc::clone(&pm),
            )
            .await?;
        }
    }
    Ok(())
}

async fn remove_obsolete_tasks(
    modules: Vec<String>,
    store: Arc<SQLiteStore>,
    mut task_container: DeployPhaseTasks,
    config: Arc<DotdeployConfig>,
    pm: Arc<PrivilegeManager>,
) -> Result<DeployPhaseTasks> {
    let mut set = JoinSet::new();
    for module in modules.into_iter() {
        let store = Arc::clone(&store);
        set.spawn(async move {
            let uuids = store.get_task_uuids(&module).await;
            uuids
        });
    }
    let stored_uuids = set.join_all().await;

    let stored_uuids = errors::join_errors(stored_uuids)?
        .into_iter()
        .flatten()
        .collect::<HashSet<_>>();

    let mut set = JoinSet::new();
    for task in task_container.tasks.into_iter() {
        set.spawn(async move {
            let uuid = task.calculate_uuid().await;
            (task, uuid)
        });
    }
    let (tasks, uuids): (Vec<_>, Vec<_>) = set.join_all().await.into_iter().unzip();
    task_container.tasks = tasks;
    let uuids = errors::join_errors(uuids)?
        .into_iter()
        .collect::<HashSet<_>>();

    // Run remove and also remove from store
    let mut obsolete_tasks = vec![];
    for uuid in stored_uuids.difference(&uuids) {
        if let Some(task) = store.get_task(*uuid).await? {
            obsolete_tasks.push(task);
            store.remove_task(*uuid).await?;
        }
    }
    if !obsolete_tasks.is_empty() {
        warn!(
            "{} {} not part of the configuration anymore or {} changed. Removing",
            obsolete_tasks.len(),
            if obsolete_tasks.len() > 1 {
                "tasks are"
            } else {
                "task is"
            },
            if obsolete_tasks.len() > 1 {
                "their definitons have"
            } else {
                "its definiton has"
            }
        );

        let mut obsolete_tasks = DeployPhaseTasks {
            tasks: obsolete_tasks,
        };
        obsolete_tasks
            .exec_pre_tasks(&pm, &config, DeployPhase::Remove)
            .await?;
        obsolete_tasks
            .exec_post_tasks(&pm, &config, DeployPhase::Remove)
            .await?;

        info!("Finished removal of obsolete tasks")
    }
    Ok(task_container)
}
