use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use crate::common;

async fn remove_file<S: AsRef<str>>(
    file: S,
    stores: Arc<(crate::cache::Store, Option<crate::cache::Store>)>,
) -> Result<()> {
    if common::check_file_exists(file.as_ref()).await? {
        // Delete file
        common::delete_file(file.as_ref()).await?;
        debug!("Removed {:?}", file.as_ref());

        // Check if we have a backup in the user store
        if stores
            .0
            .check_backup_exists(file.as_ref())
            .await
            .map_err(|e| e.into_anyhow())?
        {
            // Restore backup
            stores
                .0
                .restore_backup(file.as_ref(), file.as_ref())
                .await
                .map_err(|e| e.into_anyhow())?;
            // TODO validate restored backup
            // Remove backup from store
            stores
                .0
                .remove_backup(file.as_ref())
                .await
                .map_err(|e| e.into_anyhow())?;

            info!("Restored {:?} from backup", file.as_ref());
        }

        // Check if we have a backup in the system store
        if let Some(sys_store) = &stores.1 {
            if sys_store
                .check_backup_exists(file.as_ref())
                .await
                .map_err(|e| e.into_anyhow())?
            {
                // Restore backup
                sys_store
                    .restore_backup(file.as_ref(), file.as_ref())
                    .await
                    .map_err(|e| e.into_anyhow())?;
                // TODO validate restored backup
                // Remove backup from store
                sys_store
                    .remove_backup(file.as_ref())
                    .await
                    .map_err(|e| e.into_anyhow())?;

                info!("Restored {:?} from backup", file.as_ref());
            }
        }
    }
    Ok(())
}

pub(crate) async fn remove(
    mut phases: BTreeMap<String, crate::phases::Phase>,
    stores: Arc<(crate::cache::Store, Option<crate::cache::Store>)>,
    files: Vec<crate::cache::StoreFile>,
    dotdeploy_config: &crate::config::ConfigFile,
) -> Result<()> {
    let phase_name = "remove";
    info!("Starting {} phase", phase_name.to_uppercase());
    // We can consume the phases BTreeMap, thus remove the key from it and take ownership.
    if let Some(phase) = phases.remove(phase_name) {
        // Extract actions
        // Directly extract the inner fields if permissions is Some, otherwise set them to None
        let (pre_actions, main_actions, post_actions) =
            phase.actions.map_or((None, None, None), |mut map| {
                (map.remove("pre"), map.remove("main"), map.remove("post"))
            });

        if let Some(v) = pre_actions {
            if !v.is_empty() {
                info!("Executing pre stage actions");
                for a in v.into_iter() {
                    a.run().await?
                }
            }
        }

        if let Some(packages) = phase.packages {
            // Get default commands
            let default_cmds = crate::packages::default_cmds()?.1;
            let mut install_cmd: VecDeque<String> = VecDeque::new();
            if let Some(cmd) = &dotdeploy_config.remove_pkg_cmd {
                install_cmd = cmd.clone();
            } else if let Some(cmd) = default_cmds.get(&dotdeploy_config.distribution) {
                install_cmd = cmd.clone()
            } else {
                bail!("Failed to get package install command")
            }
            if let Some(cmd) = install_cmd.pop_front() {
                // Add packages
                for pkg in packages.into_iter() {
                    install_cmd.push_back(pkg);
                }
                let mut cmd = tokio::process::Command::new(&cmd)
                    .args(&install_cmd)
                    .spawn()
                    .with_context(|| {
                        format!("Failed to spawn {:?} with args: {:?}", cmd, install_cmd)
                    })?;

                if cmd.wait().await?.success() {
                    
                } else {
                    bail!("Failed to execute {:?} with args: {:?}", cmd, install_cmd)
                }
            }
        }

        warn!("This shit better works...");
        let mut set = tokio::task::JoinSet::new();

        // Remove the file async
        for file in files.clone() {
            let stores_clone = Arc::clone(&stores); // Clone the Arc
            set.spawn(async move {
                match remove_file(&file.destination, stores_clone).await {
                    Ok(()) => Ok(()),
                    Err(e) => bail!("Failed to remove {:?}\n {:?}", &file.destination, e),
                }
            });
        }

        while let Some(res) = set.join_next().await {
            res??;
        }

        // But remove the parent directories sync!
        for file in files {
            // Remove directory and parents if empty
            common::delete_parents(&file.destination, false).await?;
        }

        if let Some(v) = main_actions {
            if !v.is_empty() {
                info!("Executing main stage actions");
                for a in v.into_iter() {
                    a.run().await?
                }
            }
        }

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
