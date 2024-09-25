//! This module handles the removal process for files and packages, including backup restoration and
//! cleanup operations.

use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use crate::utils::file_fs;

/// Removes a file and restores its backup if available.
///
/// This function deletes the specified file and attempts to restore its backup from either the user
/// or system store.
///
/// # Arguments
///
/// * `file` - The path of the file to remove
/// * `stores` - Arc-wrapped tuple of database stores (user and optional system store)
///
/// # Returns
///
/// A Result indicating success or failure of the file removal and backup restoration process
async fn remove_file<S: AsRef<str>>(
    file: S,
    stores: Arc<(crate::store::db::Store, Option<crate::store::db::Store>)>,
) -> Result<()> {
    if file_fs::check_file_exists(file.as_ref()).await? {
        // Delete the file
        file_fs::delete_file(file.as_ref()).await?;
        debug!("Removed {:?}", file.as_ref());

        // Check for and restore backup from the user store
        if stores
            .0
            .check_backup_exists(file.as_ref())
            .await
            .map_err(|e| e.into_anyhow())?
        {
            stores
                .0
                .restore_backup(file.as_ref(), file.as_ref())
                .await
                .map_err(|e| e.into_anyhow())?;
            // TODO: Implement backup validation
            stores
                .0
                .remove_backup(file.as_ref())
                .await
                .map_err(|e| e.into_anyhow())?;

            info!("Restored {:?} from user store backup", file.as_ref());
        }

        // Check for and restore backup from the system store (if it exists)
        if let Some(sys_store) = &stores.1 {
            if sys_store
                .check_backup_exists(file.as_ref())
                .await
                .map_err(|e| e.into_anyhow())?
            {
                sys_store
                    .restore_backup(file.as_ref(), file.as_ref())
                    .await
                    .map_err(|e| e.into_anyhow())?;
                // TODO: Implement backup validation
                sys_store
                    .remove_backup(file.as_ref())
                    .await
                    .map_err(|e| e.into_anyhow())?;

                info!("Restored {:?} from system store backup", file.as_ref());
            }
        }
    }
    Ok(())
}

/// Executes the removal process for files and packages.
///
/// This function handles the "remove" phase, including pre-actions, package removal, file removal,
/// and post-actions.
///
/// # Arguments
///
/// * `phases` - A BTreeMap of phase names to their corresponding Phase structs
/// * `stores` - Arc-wrapped tuple of database stores (user and optional system store)
/// * `files` - A vector of StoreFile objects representing files to be removed
/// * `dotdeploy_config` - Configuration for the deployment process
///
/// # Returns
///
/// A Result indicating success or failure of the overall removal process
pub(crate) async fn remove(
    mut phases: BTreeMap<String, crate::phases::Phase>,
    stores: Arc<(crate::store::db::Store, Option<crate::store::db::Store>)>,
    files: Vec<crate::store::files::StoreFile>,
    dotdeploy_config: &crate::config::ConfigFile,
) -> Result<()> {
    let phase_name = "remove";
    info!("Starting {} phase", phase_name.to_uppercase());

    // Extract the "remove" phase from the phases BTreeMap
    if let Some(phase) = phases.remove(phase_name) {
        // Extract actions for pre, main, and post stages
        let (pre_actions, main_actions, post_actions) =
            phase.actions.map_or((None, None, None), |mut map| {
                (map.remove("pre"), map.remove("main"), map.remove("post"))
            });

        // Execute pre-stage actions
        if let Some(v) = pre_actions {
            if !v.is_empty() {
                info!("Executing pre stage actions");
                for a in v.into_iter() {
                    a.run().await?
                }
            }
        }

        // Handle package removal
        if let Some(packages) = phase.packages {
            // Prepare package removal command
            let default_cmds = crate::packages::default_cmds()?.1;
            let mut remove_cmd: VecDeque<String> = VecDeque::new();

            // Determine the removal command based on config or default
            if let Some(cmd) = &dotdeploy_config.remove_pkg_cmd {
                remove_cmd = cmd.clone();
            } else if let Some(cmd) = default_cmds.get(&dotdeploy_config.distribution) {
                remove_cmd = cmd.clone()
            } else {
                bail!("Failed to get package removal command")
            }

            // Execute package removal
            if let Some(cmd) = remove_cmd.pop_front() {
                // Add packages to the removal command
                for pkg in packages.into_iter() {
                    remove_cmd.push_back(pkg);
                }

                // Spawn the removal process
                let mut cmd = tokio::process::Command::new(&cmd)
                    .args(&remove_cmd)
                    .spawn()
                    .with_context(|| {
                        format!("Failed to spawn {:?} with args: {:?}", cmd, remove_cmd)
                    })?;

                // Check if the removal was successful
                if !cmd.wait().await?.success() {
                    bail!("Failed to execute {:?} with args: {:?}", cmd, remove_cmd)
                }
            }
        }

        warn!("This shit better works...");  // TODO: Consider removing or rephrasing this debug message
        let mut set = tokio::task::JoinSet::new();

        // Remove files asynchronously
        for file in files.clone() {
            let stores_clone = Arc::clone(&stores);
            set.spawn(async move {
                match remove_file(&file.destination, stores_clone).await {
                    Ok(()) => Ok(()),
                    Err(e) => bail!("Failed to remove {:?}\n {:?}", &file.destination, e),
                }
            });
        }

        // Wait for all file removal tasks to complete
        while let Some(res) = set.join_next().await {
            res??;
        }

        // Remove parent directories synchronously
        for file in files {
            file_fs::delete_parents(&file.destination, false).await?;
        }

        // Execute main-stage actions
        if let Some(v) = main_actions {
            if !v.is_empty() {
                info!("Executing main stage actions");
                for a in v.into_iter() {
                    a.run().await?
                }
            }
        }

        // Execute post-stage actions
        if let Some(v) = post_actions {
            if !v.is_empty() {
                info!("Executing post stage actions");
                for a in v.into_iter() {
                    a.run().await?
                }
            }
        }
    }
    info!("Finished {} phase", phase_name.to_uppercase());
    Ok(())
}
