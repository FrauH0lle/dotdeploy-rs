//! This module handles the deployment process, executing phases and their associated actions, file
//! operations, and package installations.

use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

/// Executes the deployment process, including setup, deployment, and configuration phases.
///
/// This function iterates through predefined phases, executing actions, handling file operations,
/// and managing package installations for each phase.
///
/// # Arguments
///
/// * `phases` - A BTreeMap of phase names to their corresponding Phase structs
/// * `stores` - Arc-wrapped tuple of database stores (user and optional system store)
/// * `context` - JSON context for template rendering
/// * `hb` - Handlebars instance for template rendering
/// * `dotdeploy_config` - Configuration for the deployment process
///
/// # Returns
///
/// A Result indicating success or failure of the overall deployment process
pub(crate) async fn deploy(
    mut phases: BTreeMap<String, crate::phases::Phase>,
    stores: Arc<(crate::store::db::Store, Option<crate::store::db::Store>)>,
    context: serde_json::Value,
    hb: Arc<handlebars::Handlebars<'static>>,
    dotdeploy_config: &crate::config::ConfigFile,
) -> Result<()> {
    let hb = Arc::new(hb);
    let context = Arc::new(context);

    // Iterate through predefined phases: setup, deploy, and config
    for phase_name in ["setup", "deploy", "config"].iter() {
        info!("Starting {} phase", phase_name.to_uppercase());

        // Remove the current phase from the BTreeMap to take ownership
        if let Some(phase) = phases.remove(*phase_name) {
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

            // Handle file operations
            if let Some(files) = phase.files {
                let mut set = tokio::task::JoinSet::new();

                // Spawn concurrent tasks for each file operation
                for file in files {
                    let stores_clone = Arc::clone(&stores);
                    let hb_clone = Arc::clone(&hb);
                    let context_clone = Arc::clone(&context);
                    set.spawn(async move {
                        file.perform(&stores_clone, &context_clone, &hb_clone).await
                    });
                }

                // Wait for all file operations to complete
                while let Some(res) = set.join_next().await {
                    res??;
                }
            }

            // Handle package installations
            if let Some(packages) = phase.packages {
                if dotdeploy_config.skip_pkg_install {
                    warn!("Skipping package installation as requested")
                } else {
                    // Prepare package installation command
                    let default_cmds = crate::packages::default_cmds()?.0;
                    let mut install_cmd: VecDeque<String> = VecDeque::new();

                    // Determine the installation command based on config or default
                    if let Some(cmd) = &dotdeploy_config.intall_pkg_cmd {
                        install_cmd = cmd.clone();
                    } else if let Some(cmd) = default_cmds.get(&dotdeploy_config.distribution) {
                        install_cmd = cmd.clone()
                    } else {
                        bail!("Failed to get package install command")
                    }

                    // Execute package installation
                    if let Some(cmd) = install_cmd.pop_front() {
                        // Add packages to the installation command
                        for pkg in packages.into_iter() {
                            install_cmd.push_back(pkg);
                        }

                        // Spawn the installation process
                        let mut cmd = tokio::process::Command::new(&cmd)
                            .args(&install_cmd)
                            .spawn()
                            .with_context(|| {
                                format!("Failed to spawn {:?} with args: {:?}", cmd, install_cmd)
                            })?;

                        // Check if the installation was successful
                        if !cmd.wait().await?.success() {
                            bail!("Failed to execute {:?} with args: {:?}", cmd, install_cmd)
                        }
                    }
                }
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
    }
    Ok(())
}
