use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

pub(crate) async fn deploy(
    mut phases: BTreeMap<String, crate::phases::Phase>,
    stores: Arc<(crate::cache::Store, Option<crate::cache::Store>)>,
    context: serde_json::Value,
    hb: Arc<handlebars::Handlebars<'static>>,
    dotdeploy_config: &crate::config::ConfigFile,
) -> Result<()> {
    let hb = Arc::new(hb);
    let context = Arc::new(context);

    for phase_name in ["setup", "deploy", "config"].iter() {
        info!("Starting {} phase", phase_name.to_uppercase());
        // We can consume the phases BTreeMap, thus remove the key from it and take ownership.
        if let Some(phase) = phases.remove(*phase_name) {
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

            if let Some(files) = phase.files {
                let mut set = tokio::task::JoinSet::new();

                for file in files {
                    let stores_clone = Arc::clone(&stores); // Clone the Arc
                    let hb_clone = Arc::clone(&hb);
                    let context_clone = Arc::clone(&context);
                    set.spawn(async move {
                        file.perform(&stores_clone, &context_clone, &hb_clone).await
                    });
                }

                while let Some(res) = set.join_next().await {
                    res??;
                }
            }

            if let Some(packages) = phase.packages {
                if dotdeploy_config.skip_pkg_install {
                    warn!("Skipping package installation as requested")
                } else {
                    // Get default commands
                    let default_cmds = crate::packages::default_cmds()?.0;
                    let mut install_cmd: VecDeque<String> = VecDeque::new();
                    if let Some(cmd) = &dotdeploy_config.intall_pkg_cmd {
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
    }
    Ok(())
}
