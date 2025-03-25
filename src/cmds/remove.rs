use crate::cmds::common;
use crate::phases::DeployPhaseStruct;
use crate::store::Store;
use crate::utils::FileUtils;
use crate::{
    config::DotdeployConfig, modules::queue::ModulesQueueBuilder, store::Stores,
    utils::sudo::PrivilegeManager,
};
use color_eyre::Result;
use color_eyre::eyre::WrapErr;
use handlebars::Handlebars;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::sync::Arc;
use toml::Value;
use tracing::{debug, error, info, warn};

/// Remove deployed modules and clean up related resources
///
/// Uninstalls specified modules and their dependencies, removes managed files, restores backups,
/// and cleans up package installations. Prevents removal of manually installed modules if not
/// explicitly requested.
///
/// * `modules` - List of modules to remove
/// * `config` - Application configuration containing removal settings  
/// * `stores` - Combined user/system store instances to modify
/// * `context` - Template context for regenerating remaining files
/// * `handlebars` - Handlebars instance for template processing
/// * `pm` - Privilege manager for elevated permissions
///
/// # Errors
/// Returns errors if:
/// * Module dependencies can't be resolved
/// * Store operations fail
/// * File operations fail
/// * Permission escalation fails
pub(crate) async fn remove(
    modules: Vec<String>,
    config: Arc<DotdeployConfig>,
    stores: Arc<Stores>,
    mut context: HashMap<String, Value>,
    handlebars: Handlebars<'static>,
    pm: Arc<PrivilegeManager>,
) -> Result<bool> {
    // Get module dependencies
    let mut full_modules = HashSet::new();

    for m in modules.iter() {
        // REVIEW 2025-03-23: Only user store or both?
        if let Some(st_mod) = stores.user_store.get_module(&m).await? {
            full_modules.insert(m.clone());
            if let Some(deps) = st_mod.depends {
                for d in deps.into_iter() {
                    full_modules.insert(d);
                }
            }
        } else {
            warn!("{} is not deployed", &m);
        }
    }

    if full_modules.is_empty() {
        error!("No deployed module specified. Aborting");
        return Ok(false);
    }
    // Add special module for removal
    full_modules.insert("__dotdeploy_generated".to_string());

    // FIXME 2025-03-24: Maybe merge with above?
    let mut manual_modules = vec![];
    for m in full_modules.iter() {
        // REVIEW 2025-03-23: Only user store or both?
        if let Some(st_mod) = stores.user_store.get_module(&m).await? {
            if st_mod.reason.as_str() == "manual" && !modules.contains(&st_mod.name) {
                manual_modules.push(st_mod.name);
            }
        }
    }

    if !manual_modules.is_empty() {
        warn!(
            "The following modules were installed explicitly and need to be removed explicitly:\n{}\n\n{}",
            format!(" - {}", manual_modules.join("\n - ")),
            "Aborting"
        );
        return Ok(false);
    };

    // FIXME 2025-03-23: Update command cache for removal AND update. Remove deleted modules.
    let mut tasks = if let Some(mut cached_remove_tasks) =
        stores.user_store.get_cached_commands("remove").await?
    {
        cached_remove_tasks
            .tasks
            .retain(|t| full_modules.contains(&t.module_name));
        cached_remove_tasks
    } else {
        DeployPhaseStruct {
            files: vec![],
            tasks: vec![],
        }
    };

    // Pre hook
    tasks.exec_pre_tasks(&pm, &config).await?;

    // Remove packages
    if config.remove_pkg_cmd.is_some() {
        let mut pkgs = vec![];
        for m in full_modules.iter() {
            let mut m_pkgs = stores.get_all_module_packages(m).await?;
            // Remove packages from store
            for p in m_pkgs.iter() {
                stores.remove_package(&m, &p).await?;
            }
            pkgs.append(&mut m_pkgs);
        }
        let pkgs = pkgs.into_iter().map(OsString::from).collect::<Vec<_>>();
        if !pkgs.is_empty() {
            common::exec_package_cmd(config.remove_pkg_cmd.as_ref().unwrap(), &pkgs, &pm).await?;
        }
    }

    // Remove files and restores backups
    let mut files = vec![];
    for m in full_modules.iter() {
        files.append(&mut stores.get_all_files(m).await?);
    }
    let file_utils = FileUtils::new(Arc::clone(&pm));
    for f in files {
        info!("Removing {}", &f.target);
        file_utils.delete_file(&f.target).await?;

        debug!("Looking for backup of {} to restore", &f.target);
        if stores.check_backup_exists(&f.target).await? {
            debug!("Found backup of {}", &f.target);
            info!("Restoring backup of {}", &f.target);
            stores.restore_backup(&f.target, &f.target).await?;
            stores.remove_backup(&f.target).await?;
        }

        // Delete potentially empty directories
        file_utils
            .delete_parents(&f.target, config.noconfirm)
            .await?;
    }

    // Post hook
    tasks.exec_post_tasks(&pm, &config).await?;

    // Drop modules from the store
    for m in full_modules.iter() {
        stores.remove_module(&m).await?;
    }

    // Update command cache
    for cmd in ["remove", "update"] {
        let cached_tasks =
            if let Some(mut cached_tasks) = stores.user_store.get_cached_commands(cmd).await? {
                cached_tasks
                    .tasks
                    .retain(|t| !full_modules.contains(&t.module_name));
                cached_tasks
            } else {
                DeployPhaseStruct {
                    files: vec![],
                    tasks: vec![],
                }
            };

        stores.user_store.cache_command(cmd, cached_tasks).await?;
    }

    // Update generated files
    // REVIEW 2025-03-23: Maybe there is a better way to get the required information?

    // Queue up left modules
    let modules_left = stores
        .get_all_modules()
        .await?
        .into_iter()
        .map(|m| m.name)
        .collect::<Vec<_>>();

    if !modules_left.is_empty() {
        let mut mod_queue = ModulesQueueBuilder::new()
            .with_modules(modules_left)
            .build(&config)?;

        mod_queue
            .collect_context(&mut context)
            .wrap_err("Failed to collect context")?;
        mod_queue.finalize(&context, &handlebars)?;

        let (_, _, _, _, _, file_generators, _) = mod_queue.process(Arc::clone(&config)).await?;

        debug!("Generating files");
        for file in file_generators {
            file.generate_file(&stores, &context, &handlebars, &config, Arc::clone(&pm))
                .await?;
        }
        debug!("Generating files complete");
    }

    Ok(true)
}
