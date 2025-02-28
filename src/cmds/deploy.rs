use crate::logs::log_output;
use crate::store::Store;
use crate::store::sqlite_files::StoreFile;
use crate::store::sqlite_modules::StoreModule;
use crate::utils::commands::exec_output;
use crate::utils::{FileUtils, file_fs};
use crate::{
    config::DotdeployConfig, modules::queue::ModulesQueueBuilder, store::Stores,
    utils::sudo::PrivilegeManager,
};
use color_eyre::eyre::{WrapErr, eyre};
use color_eyre::{Report, Result};
use handlebars::Handlebars;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::task::JoinSet;
use toml::Value;
use tracing::{debug, info, instrument, warn};

// deploy command
//
// Is an async function
// Accepts vec of module names as input
// Further needed args: stores, context, handlebars, privilege manager
// If no module name provided from CLI, default to "hosts/HOSTNAME" (HOSTNAME from dotdeploy config
// struct)
//
// Init modulequeue with module names
// call collect module names
// call collect context
// call finalize
// call process and get setup, deploy, config and remove phase
//
// For every deployed file of, check if
//  a) target checksum still corresponds to target checksum in store (copy type)
//  b) source checksum still corresponds to source checksum in store (copy type)
//  c) the source file still exists (copy, link and create type)
// If file does not exist anymore, remove file and restore backup.
// If target checksum does not correspond to target checksum in store, it means that it was modified
// outside of dotdeploy.
//  -> Issue warning that contents will be overwritten and ask user for confirmation
//  -> Refer to sync command
// If source checksums differ it means that it should be updated.
// If all conditions hold for a copy file, it means it is up-to-date and can be removed from the
// deployment queue. Same for link which have a valid source -> target relationship.
// Create files should simply always be generated anew or deleted if their source is gone.
// Operation should be async.
//
// Group files into files going into the user's home directory and other in order to select the
// correct store.
//
// for each phase in setup, deploy and config
//  - run pre step tasks (sync)
//  - deploy files (async, in setup and config phase) or install packages (only deploy phase)
//   - Needs copy, lind and create methods
//   - copy and create should run template expansion
//  - run post step tasks (sync)
// Idea: use temp_env to set DOD_CURRENT_MODULE during tasks.
// Tasks should be sync because we cannot control what is happenig so just run them in order.
//
// Store the tasks from remove and the messages of remove type in store. They should be used by the
// remove command. Drop any present ones in store before.
//
// Run generators for all module in store.
// Display messages of deploy type.

#[instrument(level = "trace")]
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

    mod_queue
        .collect_module_names(&mut context)
        .wrap_err("Failed to collect module names")?;
    mod_queue
        .collect_context(&mut context)
        .wrap_err("Failed to collect context")?;
    mod_queue.finalize(&context, &handlebars)?;

    // Ensure modules are set
    for module in mod_queue.modules.iter() {
        stores
            .add_module(&StoreModule::new(
                module.name.clone(),
                file_fs::path_to_string(&module.location)?,
                Some(whoami::username()),
                module.reason.clone(),
                module.depends_on.clone(),
                chrono::offset::Utc::now(),
            ))
            .await?
    }

    let (mut setup_phase, mut config_phase, packages, file_generators, module_messages) =
        mod_queue.process(Arc::clone(&config)).await?;

    let op_tye = "copy";

    // Insert a module
    for test_m in vec!["test1", "test2", "test3"] {
        for i in 0..5 {
            let local_time = chrono::offset::Utc::now();
            let test_file = StoreFile::new(
                test_m.to_string(),
                match op_tye {
                    "link" => Some(format!("/dotfiles/foo{}.txt", i)),
                    "copy" => Some(format!("/dotfiles/foo{}.txt", i)),
                    "create" => None,
                    _ => {
                        return Err(eyre!(
                            "Invalid 'which' parameter. Must be either 'link', 'copy' or 'create'."
                        ));
                    }
                },
                match op_tye {
                    "link" => Some(format!("source_checksum{}", i)),
                    "copy" => Some(format!("source_checksum{}", i)),
                    "create" => None,
                    _ => {
                        return Err(eyre!(
                            "Invalid 'which' parameter. Must be either 'link', 'copy' or 'create'."
                        ));
                    }
                },
                format!("/home/{}/foo{}.txt", test_m, i),
                Some(format!("dest_checksum{}", i)),
                match op_tye {
                    "link" => "link".to_string(),
                    "copy" => "copy".to_string(),
                    "create" => "create".to_string(),
                    _ => {
                        return Err(eyre!(
                            "Invalid 'which' parameter. Must be either 'link', 'copy' or 'create'."
                        ));
                    }
                },
                Some("user".to_string()),
                local_time,
            );

            stores.user_store.add_file(test_file.clone()).await?;
            stores
                .system_store
                .as_ref()
                .unwrap()
                .add_file(test_file)
                .await?;
        }
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
    setup_phase.exec_pre_tasks(&*pm, &*config).await?;
    setup_phase
        .deploy_files(
            Arc::clone(&pm),
            Arc::clone(&stores),
            Arc::clone(&context),
            Arc::clone(&hb),
        )
        .await?;
    setup_phase.exec_post_tasks(&*pm, &*config).await?;
    debug!("SETUP phase complete");

    // Install packages
    //
    // FIXME 2025-03-20: The checks if packages should be installed need to happen before the
    //   deployment starts.

    if config.skip_pkg_install {
        info!("Skipping package installation as requested");
    } else if !packages.is_empty() {
        if config.install_pkg_cmd.is_none() {
            warn!(
                "Found packages to install, but `install_pkg_cmd` in config is not defined! Skipping package installation."
            );
        } else {
            info!("Installing packages");

            // Verify installed packages
            let mut obsolete = vec![];
            let pkg_modules = packages
                .iter()
                .map(|x| &x.module_name)
                .collect::<HashSet<_>>();

            // For each module, get all registered packages
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
                // FIXME 2025-03-21: Implement removal
                let diff = store_pkgs
                    .difference(&requested_pkgs)
                    .collect::<HashSet<_>>();
                let other_module_pkgs = stores.get_all_other_module_packages(&pmod).await?;
                // Drop packages for module
                for p in diff {
                    stores.remove_package(pmod, &p).await?;
                    if !other_module_pkgs.contains(&pmod) {
                        obsolete.push(p.to_string());
                    }
                }
            }

            // Remove obsolete packages

            // REVIEW 2025-03-21: Remove empty string
            obsolete.retain(|p| !p.is_empty());
            if !obsolete.is_empty() {
                let remove_cmd = config.remove_pkg_cmd.as_ref().unwrap();
                let first_cmd = &remove_cmd[0];
                dbg!(&remove_cmd);
                if *first_cmd == pm.root_cmd.cmd().to_string_lossy() {
                    let output = pm
                        .sudo_exec_output(
                            &remove_cmd[1],
                            &vec![&remove_cmd[2..], &obsolete].concat(),
                            Some("Installing packages"),
                        )
                        .await?;

                    log_output!(output.stdout, "Stdout", remove_cmd.join(" "), info);
                    log_output!(output.stderr, "Stderr", remove_cmd.join(" "), info);

                    if !output.status.success() {
                        return Err(eyre!("Failed to install packages"));
                    }
                } else {
                    let output =
                        exec_output(&remove_cmd[0], &vec![&remove_cmd[1..], &obsolete].concat())
                            .await?;

                    log_output!(output.stdout, "Stdout", remove_cmd.join(" "), info);
                    log_output!(output.stderr, "Stderr", remove_cmd.join(" "), info);

                    if !output.status.success() {
                        return Err(eyre!("Failed to install packages"));
                    }
                }
            }

            // Add packages to store

            // REVIEW 2025-03-21: Remove empty string
            let packages = packages
                .into_iter()
                .filter(|p| !p.package.is_empty())
                .collect::<Vec<_>>();

            for p in packages.iter() {
                stores.add_package(&p.module_name, &p.package).await?
            }

            let packages = packages.into_iter().map(|p| p.package).collect::<Vec<_>>();

            if !packages.is_empty() {
                let install_cmd = config.install_pkg_cmd.as_ref().unwrap();
                let first_cmd = &install_cmd[0];
                dbg!(&install_cmd);
                if *first_cmd == pm.root_cmd.cmd().to_string_lossy() {
                    let output = pm
                        .sudo_exec_output(
                            &install_cmd[1],
                            &vec![&install_cmd[2..], &packages].concat(),
                            Some("Installing packages"),
                        )
                        .await?;

                    log_output!(output.stdout, "Stdout", install_cmd.join(" "), info);
                    log_output!(output.stderr, "Stderr", install_cmd.join(" "), info);

                    if !output.status.success() {
                        return Err(eyre!("Failed to install packages"));
                    }
                } else {
                    let output = exec_output(
                        &install_cmd[0],
                        &vec![&install_cmd[1..], &packages].concat(),
                    )
                    .await?;

                    log_output!(output.stdout, "Stdout", install_cmd.join(" "), info);
                    log_output!(output.stderr, "Stderr", install_cmd.join(" "), info);

                    if !output.status.success() {
                        return Err(eyre!("Failed to install packages"));
                    }
                }
            }

            info!("Package installation complete");
        }
    }

    debug!("Running CONFIG phase");
    config_phase.exec_pre_tasks(&*pm, &*config).await?;
    config_phase
        .deploy_files(
            Arc::clone(&pm),
            Arc::clone(&stores),
            Arc::clone(&context),
            Arc::clone(&hb),
        )
        .await?;
    config_phase.exec_post_tasks(&*pm, &*config).await?;
    debug!("CONFIG phase complete");

    // Generate files
    debug!("Generating files");
    for file in file_generators {
        file.generate_file(&*stores, &*context, &*hb, &*config, Arc::clone(&pm))
            .await?;
    }
    debug!("Generating files complete");

    // Display messages
    debug!("Displaying messages");
    for msg in module_messages
        .into_iter()
        .filter(|m| m.on_command.as_deref() == Some("deploy"))
    {
        info!("Message for {}:\n{}", msg.module_name, msg.message)
    }

    debug!("Deploy command complete");
    Ok(true)
}

async fn collect_deployed_files(
    deployed_modules: HashSet<String>,
    stores: Arc<Stores>,
) -> Result<Vec<StoreFile>> {
    let mut set = JoinSet::new();

    for name in deployed_modules {
        set.spawn({
            let stores = Arc::clone(&stores);
            let name = name.clone();
            async move {
                let files = stores.get_all_files(&name).await?;
                Ok::<_, Report>(files)
            }
        });
    }

    let deployed_files = set
        .join_all()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, Report>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

    Ok(deployed_files)
}

async fn validate_deployed_files(
    deployed_files: Vec<StoreFile>,
    config: Arc<DotdeployConfig>,
    pm: Arc<PrivilegeManager>,
    stores: Arc<Stores>,
) -> Result<Vec<String>> {
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

    let modified_files = set
        .join_all()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, Report>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

    Ok(modified_files)
}
