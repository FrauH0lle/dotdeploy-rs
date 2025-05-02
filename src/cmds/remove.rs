use crate::cmds::common;
use crate::config::DotdeployConfig;
use crate::errors;
use crate::modules::DeployPhase;
use crate::modules::queue::ModulesQueueBuilder;
use crate::phases::DeployPhaseTasks;
use crate::store::Store;
use crate::store::sqlite::SQLiteStore;
use crate::utils::FileUtils;
use crate::utils::common::bytes_to_os_str;
use crate::utils::sudo::PrivilegeManager;
use color_eyre::eyre::WrapErr;
use color_eyre::{Report, Result};
use handlebars::Handlebars;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::task::JoinSet;
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
/// * `store` - User store instances to modify
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
    store: Arc<SQLiteStore>,
    mut context: HashMap<String, Value>,
    handlebars: Handlebars<'static>,
    pm: Arc<PrivilegeManager>,
) -> Result<bool> {
    // Get module dependencies
    let mut full_modules = HashSet::new();

    // Recursively collect all dependencies
    //
    // This function needs to return a Pin<Box<dyn Future>> because it contains recursive async
    // calls. The Pin ensures the Future cannot be moved in memory once polled, which is required
    // for self-referential futures created by async/await. The Box provides the size information at
    // compile time that would otherwise be impossible to determine due to the recursive nature of
    // the future chain.
    fn collect_deps<'a>(
        module: &'a str,
        store: &'a SQLiteStore,
        collected: &'a mut HashSet<String>,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
        Box::pin(async move {
            if let Some(st_mod) = store.get_module(module).await? {
                collected.insert(module.to_string());
                if let Some(deps) = st_mod.depends {
                    for dep in deps {
                        if !collected.contains(&dep) {
                            collect_deps(&dep, store, collected).await?;
                        }
                    }
                }
            } else {
                warn!("{} is not deployed", module);
            }
            Ok(())
        })
    }

    for m in modules.iter() {
        collect_deps(m, &store, &mut full_modules).await?;
    }

    if full_modules.is_empty() {
        error!("No deployed module specified. Aborting");
        return Ok(false);
    }
    // Add special module for removal
    full_modules.insert("__dotdeploy_generated".to_string());

    // Check if a explicitly installed module should be removed as a dependency but was not
    // explicitly requested by the user
    let mut manual_modules = vec![];
    for m in full_modules.iter() {
        if let Some(st_mod) = store.get_module(&m).await? {
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

    warn!(
        "The following modules will be removed:{}",
        format!(
            "\n  - {}",
            full_modules
                .iter()
                .filter(|m| *m != "__dotdeploy_generated")
                .map(|m| m.as_str())
                .collect::<Vec<_>>()
                .join("\n  - ")
        ),
    );

    if !(config.force
        || crate::utils::common::ask_boolean(&format!(
            "{}\n{}",
            "Do you want to remove these modules? [y/N]?",
            "(You can skip this prompt with the CLI argument '-f/--force')"
        )))
    {
        error!("Aborted");
        return Ok(false);
    }

    let mut set = JoinSet::new();
    for module in full_modules.into_iter() {
        let store = Arc::clone(&store);
        set.spawn(async move {
            let tasks = store.get_tasks(&module).await?;
            Ok((module, tasks))
        });
    }
    let (mut full_modules, tasks): (HashSet<_>, Vec<_>) =
        errors::join_errors(set.join_all().await)?
            .into_iter()
            .unzip();
    let mut tasks = DeployPhaseTasks {
        tasks: tasks.into_iter().flatten().collect(),
    };

    // Pre hook
    tasks
        .exec_pre_tasks(&pm, &config, DeployPhase::Remove)
        .await?;

    // Remove packages
    if config.remove_pkg_cmd.is_some() {
        let mut set = JoinSet::new();

        for m in full_modules
            .into_iter()
            .filter(|m| m.as_str() != "__dotdeploy_generated")
        {
            set.spawn({
                let store = Arc::clone(&store);
                async move {
                    let m_pkgs = store.get_all_module_packages(&m).await?;
                    // Remove packages from store
                    for p in m_pkgs.iter() {
                        store.remove_package(&m, p).await?;
                    }
                    Ok::<_, Report>((m, m_pkgs))
                }
            });
        }
        let (modules, pkgs): (HashSet<_>, Vec<_>) = errors::join_errors(set.join_all().await)?
            .into_iter()
            .unzip();
        let pkgs = pkgs
            .into_iter()
            .flatten()
            .map(OsString::from)
            .collect::<Vec<_>>();
        // Reassign the modules
        full_modules = modules;
        full_modules.insert("__dotdeploy_generated".to_string());

        // Remove packages
        if !pkgs.is_empty() {
            common::exec_package_cmd(config.remove_pkg_cmd.as_ref().unwrap(), &pkgs, &pm).await?;
        }
    }

    // Remove files and restores backups
    let mut files = vec![];
    for m in full_modules.iter() {
        files.append(&mut store.get_all_files(m).await?);
    }
    let file_utils = Arc::new(FileUtils::new(Arc::clone(&pm)));

    let mut set = JoinSet::new();
    let guard = Arc::new(tokio::sync::Mutex::new(()));

    for f in files {
        set.spawn({
            let store = Arc::clone(&store);
            let config = Arc::clone(&config);
            let file_utils = Arc::clone(&file_utils);
            let guard = Arc::clone(&guard);
            async move {
                let target = PathBuf::from(bytes_to_os_str(f.target_u8));
                info!("Removing {}", &target.display());
                file_utils.delete_file(&target).await?;

                if store.check_backup_exists(&target).await? {
                    store.restore_backup(&target, &target).await?;
                    store.remove_backup(&target).await?;
                }
                file_utils
                    .delete_parents(&target, config.noconfirm, Some(guard))
                    .await?;

                // Delete potentially empty directories
                Ok::<_, Report>(())
            }
        });
    }
    errors::join_errors(set.join_all().await)?;

    // Post hook
    tasks
        .exec_post_tasks(&pm, &config, DeployPhase::Remove)
        .await?;

    // Drop modules from the store
    for m in full_modules.iter() {
        store.remove_module(&m).await?;
    }

    // Update generated files

    // Queue up left modules
    let modules_left = store
        .get_all_modules()
        .await?
        .into_iter()
        .filter(|m| m.name.as_str() != "__dotdeploy_generated")
        .map(|m| m.name)
        .collect::<Vec<_>>();

    if !modules_left.is_empty() {
        let mut mod_queue = ModulesQueueBuilder::new()
            .with_modules(modules_left)
            .build(&config)?;

        // Add queued modules to context
        mod_queue
            .add_mod_names_to_context(&mut context, Arc::clone(&store))
            .await?;

        // Make modules available as the env var DOD_MODULES="mod1,mod2,mod3"
        if let Some(Value::Array(mods)) = context.get("DOD_MODULES") {
            let modules_str = mods
                .iter()
                .filter_map(|m| m.as_str())
                .collect::<Vec<_>>()
                .join(",");
            unsafe { std::env::set_var("DOD_MODULES", &modules_str) }
            debug!("Set `DOD_MODULES` to {}", &modules_str)
        }

        mod_queue
            .collect_context(&mut context)
            .wrap_err("Failed to collect context")?;
        mod_queue.finalize(&mut context, &handlebars)?;

        let file_generators = mod_queue.get_file_generators().await?;

        debug!("Generating files");

        // Wrap handlebars and context in an Arc as they will be shared across threads
        let hb = Arc::new(handlebars);
        let context = Arc::new(context);

        let mut set = JoinSet::new();
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
    }

    for m in full_modules.iter() {
        let msgs = store.get_all_cached_messages(m.as_str(), "update").await;

        if let Ok(msgs) = msgs {
            for msg in msgs.into_iter() {
                match msg.on_command.as_deref() {
                    Some("remove") => {
                        info!("Message for {}:\n{}", msg.module_name, msg.message)
                    }
                    _ => unreachable!(),
                }
            }
        }
    }

    Ok(true)
}
