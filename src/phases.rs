use anyhow::{anyhow, bail, Context, Result};

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::Ordering;

use crate::store::Stores;
use crate::phases::destination::Destination;
use crate::phases::file_operations::{FileOperation, ManagedFile};
use crate::utils::file_fs;

pub(crate) mod destination;
pub(crate) mod file_operations;

// Structs

/// Represents a processing phase like Setup, Deployment or Configuration.
#[derive(Debug)]
pub(crate) struct Phase {
    /// Files to deploy during the phase.
    pub(crate) files: Option<VecDeque<ManagedFile>>,
    /// Actions to be executed during the stages.
    pub(crate) actions: Option<BTreeMap<String, Vec<crate::modules::actions::ModuleAction>>>,
    /// Packages to install. Will be only used in the "deploy" phase.
    pub(crate) packages: Option<Vec<String>>,
}

/// Processes module configurations and assigns them to the corresponding deployment phases.
///
/// This function iterates over a set of modules, evaluates their configurations against a provided
/// context, and then assigns their file operations, actions, and packages to specified deployment
/// phases. Each module's configuration can dictate which phase its components are assigned to,
/// allowing for conditional deployment based on the context.
///
/// # Arguments
/// * `modules` - A set of modules whose configurations are to be processed and assigned.
/// * `context` - A context used for evaluating conditional configurations within each module.
///
/// # Errors
/// Returns an error if conditional evaluation fails for any module configuration, or if there's an
/// attempt to use an undefined phase or action.
pub(crate) async fn assign_module_config(
    modules: std::collections::BTreeSet<crate::modules::Module>,
    context: serde_json::Value,
    stores: &Stores,
    messages: &mut (
        std::collections::BTreeMap<String, Vec<String>>,
        std::collections::BTreeMap<String, Vec<String>>,
    ),
    generators: &mut std::collections::BTreeMap<std::path::PathBuf, crate::modules::generate::Generate>,
    hb: &handlebars::Handlebars<'static>,
) -> Result<BTreeMap<String, Phase>> {
    let mut phases: BTreeMap<String, Phase> = BTreeMap::new();
    let stage_names = ["pre", "main", "post"];

    // Initialize deployment phases with predefined stages
    for phase_name in ["setup", "deploy", "config", "remove"].iter() {
        let actions_stage_init = stage_names
            .iter()
            .map(|&stage| (stage.to_string(), Vec::new()))
            .collect();

        phases.insert(
            phase_name.to_string(),
            Phase {
                files: Some(VecDeque::new()),
                actions: Some(actions_stage_init),
                packages: match *phase_name {
                    "deploy" => Some(Vec::new()),
                    "remove" => Some(Vec::new()),
                    _ => None,
                },
            },
        );
    }

    // Iterate through each module to assign its configurations to the appropriate phase and stage.
    // BTreeSet does not have drain so we do it manually. Should be fast enough.
    for mut module in modules.into_iter() {
        // Evaluate module configurations against the provided context
        module
            .config
            .eval_conditionals(&context, hb)
            .with_context(|| {
                format!(
                    "Failed to evaluate conditionals for module '{}'",
                    module.name
                )
            })?;

        let module_name = module.name;

        // Add messages
        if let Some(mod_messages) = module.config.messages {
            for m in mod_messages.iter() {
                match m.display_when.as_str() {
                    "deploy" => {
                        if messages.0.get(&module_name).is_none() {
                            messages.0.insert(module_name.clone(), vec![]);
                        }
                        let value = messages.0.get_mut(&module_name).unwrap();
                        let rendered =
                            hb.render_template(&m.message, &context).with_context(|| {
                                format!("Failed to render template {:?}", &m.message)
                            })?;

                        value.push(rendered);
                        // messages.0.insert(module_name.clone(), value.to_vec());
                    }
                    "remove" => {
                        if messages.1.get(&module_name).is_none() {
                            messages.1.insert(module_name.clone(), vec![]);
                        }
                        let value = messages.1.get_mut(&module_name).unwrap();
                        value.push(m.message.clone());
                        let rendered =
                            hb.render_template(&m.message, &context).with_context(|| {
                                format!("Failed to render template {:?}", &m.message)
                            })?;

                        value.push(rendered);
                        // messages.0.insert(module_name.clone(), value.to_vec());
                    }
                    _ => unreachable!(),
                }
            }
        }

        // Add generators
        if let Some(mod_generators) = module.config.generate {
            for (k, v) in mod_generators.into_iter() {
                generators.insert(k, v);
            }
        }

        // Retrieve deployed files in order to check if their status is still valid. Thus, if a source
        // does not exist or it is not part of the config anymore, remove the destination.
        let mut user_files: HashMap<String, (Option<String>, String)> = stores
            .user_store
            .get_all_files(&module_name)
            .await
            .map_err(|e| e.into_anyhow())?
            .into_iter()
            .map(|f| (f.destination, (f.source, f.operation)))
            .collect();
        let mut sys_files: HashMap<String, (Option<String>, String)> =
            if let Some(ref sys_store) = stores.system_store {
                sys_store
                    .get_all_files(&module_name)
                    .await
                    .map_err(|e| e.into_anyhow())?
                    .into_iter()
                    .map(|f| (f.destination, (f.source, f.operation)))
                    .collect()
            } else {
                HashMap::new()
            };

        // Assign files to phases based on their specified deployment phase.
        if let Some(files) = module.config.files {
            assign_files_to_phases(
                module_name.clone(),
                files,
                &mut phases,
                &mut user_files,
                &mut sys_files,
            )?;
        }
        // Assign actions to their respective phases and stages.
        if let Some(actions) = module.config.actions {
            assign_actions_to_phases(actions, &mut phases)?;
        }
        // Append packages to the deploy phase, if any.
        if let Some(packages) = module.config.packages {
            append_packages(packages, &mut phases)?;
        }

        // Remove files with missing source files and files which are dynamically created.
        for (k, _) in user_files {
            info!(
                "{}: '{}' is not part of the config anymore, its action has changed or source file has been removed. Removing.",
                module_name, k
            );
            stores
                .user_store
                .remove_file(&k)
                .await
                .map_err(|e| e.into_anyhow())?;

            file_fs::delete_file(&k)
                .await
                .with_context(|| format!("Failed to remove file {:?}", &k))?;

            // Restore backup
            if stores
                .user_store
                .check_backup_exists(&k)
                .await
                .map_err(|e| e.into_anyhow())?
            {
                stores
                    .user_store
                    .restore_backup(&k, &k)
                    .await
                    .map_err(|e| e.into_anyhow())?;
                // TODO validate restored backup
                // Remove backup from store
                stores
                    .user_store
                    .remove_backup(&k)
                    .await
                    .map_err(|e| e.into_anyhow())?;

                info!("Restored {:?} from backup", &k);
            }
        }
        for (k, _) in sys_files {
            info!(
                "{}: '{}' is not part of the config anymore, its action has changed or source file has been removed. Removing.",
                module_name, k
            );

            stores
                .system_store
                .as_ref()
                .unwrap()
                .remove_file(&k)
                .await
                .map_err(|e| e.into_anyhow())?;

            file_fs::delete_file(&k)
                .await
                .with_context(|| format!("Failed to remove file {:?}", &k))?;

            // Restore backup
            if stores
                .system_store
                .as_ref()
                .unwrap()
                .check_backup_exists(&k)
                .await
                .map_err(|e| e.into_anyhow())?
            {
                stores
                    .system_store
                    .as_ref()
                    .unwrap()
                    .restore_backup(&k, &k)
                    .await
                    .map_err(|e| e.into_anyhow())?;
                // TODO validate restored backup
                // Remove backup from store
                stores
                    .system_store
                    .as_ref()
                    .unwrap()
                    .remove_backup(&k)
                    .await
                    .map_err(|e| e.into_anyhow())?;

                info!("Restored {:?} from backup", &k);
            }
        }
    }

    // TODO There should be a better way than to iterate over the phases again.
    // Remove empty elements from the phases
    for (_, phase) in phases.iter_mut() {
        // Set files to None if the vector is empty
        if phase.files.as_ref().map(|v| v.len()) == Some(0) {
            phase.files = None;
        }
        if let Some(actions) = phase.actions.as_ref() {
            if actions.values().all(|v| v.is_empty()) {
                phase.actions = None;
            }
        }
    }

    Ok(phases)
}

/// Assigns file operations from a module to their corresponding phase.
fn assign_files_to_phases(
    module_name: String,
    files: BTreeMap<PathBuf, crate::modules::files::ModuleFile>,
    phases: &mut BTreeMap<String, Phase>,
    user_files: &mut HashMap<String, (Option<String>, String)>,
    sys_files: &mut HashMap<String, (Option<String>, String)>,
) -> Result<()> {
    for (dest, conf) in files.into_iter() {
        // Check if source is defined for copy and link as well as if the file exists. If one
        // check fails, return early.
        match conf.action.as_deref() {
            Some("copy") | Some("link") => {
                let source = conf.source.clone().ok_or_else(|| {
                    anyhow!("'source' is required for 'link' or 'copy' operations")
                })?;
                match source.try_exists() {
                    Ok(true) => (),
                    _ => bail!(
                        "Source file {} either missing or its existence could not be verified!",
                        source.display()
                    ),
                }
            }
            _ => (),
        }

        let destination = if dest.starts_with(
            &shellexpand::full("$HOME")
                .context("Failed to expand $HOME")?
                .to_string(),
        ) {
            Destination::Home(PathBuf::from(
                &dest
                    .to_str()
                    .ok_or_else(|| anyhow!("Filename contains invalid Unicode characters"))?
                    // Replace ##dot## with '.' in destinations
                    .replace("##dot##", "."),
            ))
        } else if crate::DEPLOY_SYSTEM_FILES.load(Ordering::Relaxed) {
                        Destination::Root(PathBuf::from(
                            &dest
                                .to_str()
                                .ok_or_else(|| anyhow!("Filename contains invalid Unicode characters"))?
                                // Replace ##dot## with '.' in destinations
                                .replace("##dot##", "."),
                        ))
                    } else {
                        bail!(
                            "Deploying system files is disabled.
        Check the value of the variable `deploy_sys_files` in `$HOME/.config/dotdeploy/config.toml`"
                        )
                    };

        // Directly extract the inner fields if permissions is Some, otherwise set them to None
        let (owner, group, perms) = conf.permissions.map_or((None, None, None), |perms| {
            (perms.owner, perms.group, perms.permissions)
        });

        let operation = match conf.action.as_deref() {
            Some("copy") | Some("link") => {
                let source = conf.source.ok_or_else(|| {
                    anyhow!("'source' is required for 'link' or 'copy' operations")
                })?;

                // Remove files with changed source, which means here: keep them in the hashmap.
                if let Some(s) = user_files.get(&destination.path().display().to_string()) {
                    if Some(s.1.as_str()) == conf.action.as_deref() {
                        if let Some(db_source) = &s.0 {
                            if db_source == &source.display().to_string() {
                                user_files.remove(&destination.path().display().to_string());
                            }
                        }
                    }
                }
                if let Some(s) = sys_files.get(&destination.path().display().to_string()) {
                    if Some(s.1.as_str()) == conf.action.as_deref() {
                        if let Some(db_source) = &s.0 {
                            if db_source == &source.display().to_string() {
                                sys_files.remove(&destination.path().display().to_string());
                            }
                        }
                    }
                }

                match conf.action.as_deref() {
                    Some("copy") => FileOperation::Copy {
                        source,
                        destination,
                        owner: owner.map(String::from),
                        group: group.map(String::from),
                        permissions: perms.map(String::from),
                        template: conf.template.map(bool::from),
                    },
                    Some("link") => FileOperation::Symlink {
                        source,
                        destination,
                        owner: owner.map(String::from),
                        group: group.map(String::from),
                    },
                    // We've already filtered for "copy" or "link"
                    _ => unreachable!(),
                }
            }
            Some("create") => {
                let content = conf
                    .content
                    .ok_or_else(|| anyhow!("'content' is required for 'create' operations"))?;

                FileOperation::Create {
                    content,
                    destination,
                    owner: owner.map(String::from),
                    group: group.map(String::from),
                    permissions: perms.map(String::from),
                    template: conf.template,
                }
            }
            _ => return Err(anyhow!("Unsupported file action for '{}'", dest.display())),
        };

        let phase_key = conf
            .phase
            .ok_or_else(|| anyhow!("'phase' is required for file '{}'", dest.display()))?;

        if let Some(phase) = phases.get_mut(&phase_key) {
            phase.files.as_mut().unwrap().push_back(ManagedFile {
                module: module_name.clone(),
                operation,
            });
        } else {
            return Err(anyhow!(
                "Undefined phase '{}' for file '{}'",
                phase_key,
                dest.display()
            ));
        }
    }
    Ok(())
}

/// Assigns actions from a module to their corresponding phases and stages.
fn assign_actions_to_phases(
    actions: BTreeMap<String, BTreeMap<String, Vec<crate::modules::actions::ModuleAction>>>,
    phases: &mut BTreeMap<String, Phase>,
) -> Result<()> {
    // Iterate over each action's phase and its corresponding stages and actions
    for (phase_key, stages_actions) in actions.into_iter() {
        // Attempt to find the matching phase in the phases map
        if let Some(phase) = phases.get_mut(&phase_key) {
            // For each stage within the current phase, process its actions
            for (stage_name, action_vec) in stages_actions.iter() {
                // Attempt to find the stage within the phase, then add actions to it
                if let Some(phase_stage_actions) =
                    phase.actions.as_mut().and_then(|a| a.get_mut(stage_name))
                {
                    // If the stage is found, extend its list of actions with those from the module
                    phase_stage_actions.extend(action_vec.iter().cloned());
                } else {
                    // Return an error if the specified stage does not exist within the phase
                    return Err(anyhow!(
                        "Undefined stage '{}' in phase '{}'",
                        stage_name,
                        phase_key
                    ));
                }
            }
        } else {
            // Return an error if the specified phase does not exist
            return Err(anyhow!("Undefined phase '{}' for actions", phase_key));
        }
    }
    Ok(())
}

/// Appends package configurations from a module to the deploy phase.
fn append_packages(
    packages: Vec<crate::modules::packages::ModulePackages>,
    phases: &mut BTreeMap<String, Phase>,
) -> Result<()> {
    if let Some(deploy_phase) = phases.get_mut("deploy") {
        if let Some(phase_pkgs) = deploy_phase.packages.as_mut() {
            for pkg in packages.iter() {
                phase_pkgs.extend_from_slice(&pkg.install);
            }
        } else {
            return Err(anyhow!(
                "Deploy phase is missing package list initialization"
            ));
        }
    }
    if let Some(remove_phase) = phases.get_mut("remove") {
        if let Some(phase_pkgs) = remove_phase.packages.as_mut() {
            for pkg in packages.iter() {
                phase_pkgs.extend_from_slice(&pkg.install);
            }
        } else {
            return Err(anyhow!(
                "Remove phase is missing package list initialization"
            ));
        }
    }

    Ok(())
}

// // maybe rename to task?
// enum Phase {
//     Setup(Phase),
//     Deploy(Phase),
//     Config(Phase)
// }

// struct Phase {
//     pre_stage: Stage,
//     main_stage: Stage,
//     post_stag: Stagee
// }

// struct Stage {
//     // file operations
//     files: ...,
//     // packages to install (only for main stage, deploy phase)
//     packages: ...,
//     // code/programm executions
//     actions: ...,
// }
