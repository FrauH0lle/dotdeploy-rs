use anyhow::{anyhow, bail, Context, Result};

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::Ordering;

use crate::phases::destination::Destination;
use crate::phases::file_operations::FileOperation;
use crate::utils::file_fs;
use crate::utils::file_checksum;
use crate::utils::file_metadata;
use crate::utils::file_permissions;

pub(crate) mod destination;
pub(crate) mod file_operations;

// Structs

/// A structure to manage file configurations, including the operation, source and destination.
#[derive(Debug, Clone)]
pub(crate) struct ManagedFile {
    /// Module the file belongs to
    pub(crate) module: String,
    /// Which [FileOperation] to apply.
    pub(crate) operation: FileOperation,
}

impl ManagedFile {
    pub(crate) async fn perform(
        &self,
        stores: &(crate::store::db::Store, Option<crate::store::db::Store>),
        context: &serde_json::Value,
        hb: &handlebars::Handlebars<'static>,
    ) -> Result<()> {
        match &self.operation {
            FileOperation::Copy {
                source,
                destination,
                owner,
                group,
                permissions,
                template,
            } => {
                let store = match destination {
                    Destination::Home(_) => &stores.0,
                    Destination::Root(_) => {
                        stores.1.as_ref().expect("System store should not be empty")
                    }
                };

                // Perform copy operation

                // Copy when
                // - source has changed
                // - file not found in DB

                let mut do_copy = false;

                if template.expect("template should always be Some()") {
                    // Always copy the file if it is a template, no further checks
                    do_copy = true;
                } else {
                    // Check if source has changed
                    if let Some(db_src_checksum) = store
                        .get_source_checksum(destination.path())
                        .await
                        .map_err(|e| e.into_anyhow())
                        .with_context(|| {
                            format!(
                                "Failed to get source checksum for {:?} from store",
                                &destination.path()
                            )
                        })?
                    {
                        let src_checksum = file_checksum::calculate_sha256_checksum(&db_src_checksum.0)
                            .await
                            .with_context(|| {
                                format!(
                                    "Failed to get source checksum for {:?}",
                                    &db_src_checksum.0
                                )
                            })?;
                        if src_checksum != db_src_checksum.1 {
                            info!("'{}' has changed, re-deplyoing", &db_src_checksum.0);
                            do_copy = true;
                        }
                    } else {
                        info!(
                            "'{}' not found in store, deplyoing",
                            destination.path().display()
                        );
                        do_copy = true;
                    }
                }

                if do_copy {
                    // Create backup if no backup is already stored and if the destination file
                    // already exists
                    if !store
                        .check_backup_exists(destination.path())
                        .await
                        .map_err(|e| e.into_anyhow())?
                        & file_fs::check_file_exists(destination.path()).await?
                    {
                        store
                            .add_backup(destination.path())
                            .await
                            .map_err(|e| e.into_anyhow())?;
                    }
                    debug!("Trying to copy {:?} to {:?}", source, destination.path());

                    destination
                        .copy(source, *template, context, hb)
                        .await
                        .with_context(|| {
                            format!("Failed to copy {:?} to {:?}", source, destination.path())
                        })?;

                    // Set permissions
                    file_metadata::set_file_metadata(
                        destination.path(),
                        file_metadata::FileMetadata {
                            uid: owner.as_ref().map(file_permissions::user_to_uid).transpose()?,
                            gid: group.as_ref().map(file_permissions::group_to_gid).transpose()?,
                            permissions: permissions
                                .as_ref()
                                .map(file_permissions::perms_str_to_int)
                                .transpose()?,
                            is_symlink: false,
                            symlink_source: None,
                            checksum: None,
                        },
                    )
                    .await?;

                    // Record file in store
                    store
                        .add_file(crate::store::files::StoreFile {
                            module: self.module.clone(),
                            source: Some(source.display().to_string()),
                            source_checksum: Some(file_checksum::calculate_sha256_checksum(source).await?),
                            destination: destination.path().display().to_string(),
                            destination_checksum: Some(
                                file_checksum::calculate_sha256_checksum(destination.path()).await?,
                            ),
                            operation: "copy".to_string(),
                            user: Some(std::env::var("USER")?),
                            date: chrono::offset::Local::now(),
                        })
                        .await
                        .map_err(|e| e.into_anyhow())?;

                    info!(
                        "Copy: '{}' -> '{}'",
                        source.display(),
                        destination.path().display()
                    );
                } else {
                    info!("'{}' deployed and up to date", destination.path().display());
                }
            }
            FileOperation::Symlink {
                source,
                destination,
                owner,
                group,
            } => {
                let store = match destination {
                    Destination::Home(_) => &stores.0,
                    Destination::Root(_) => {
                        stores.1.as_ref().expect("System store should not be empty")
                    }
                };

                // Perform symlink operation
                if file_fs::check_file_exists(destination.path()).await?
                    && file_fs::check_link_exists(destination.path(), Some(source)).await?
                    && store
                        .check_file_exists(destination.path())
                        .await
                        .map_err(|e| e.into_anyhow())?
                {
                    info!("'{}' deployed and up to date", destination.path().display());
                } else {
                    if !store
                        .check_backup_exists(destination.path())
                        .await
                        .map_err(|e| e.into_anyhow())?
                        & file_fs::check_file_exists(destination.path()).await?
                    {
                        store
                            .add_backup(destination.path())
                            .await
                            .map_err(|e| e.into_anyhow())?;
                    }

                    debug!("Trying to link {:?} to {:?}", source, destination.path());

                    destination
                        .link(source.to_path_buf())
                        .await
                        .with_context(|| {
                            format!("Failed to link {:?} to {:?}", source, destination.path())
                        })?;

                    // Set permissions
                    file_metadata::set_file_metadata(
                        destination.path(),
                        file_metadata::FileMetadata {
                            uid: owner.as_ref().map(file_permissions::user_to_uid).transpose()?,
                            gid: group.as_ref().map(file_permissions::group_to_gid).transpose()?,
                            permissions: None,
                            is_symlink: true,
                            symlink_source: None,
                            checksum: None,
                        },
                    )
                    .await?;

                    store
                        .add_file(crate::store::files::StoreFile {
                            module: self.module.clone(),
                            source: Some(source.display().to_string()),
                            source_checksum: Some(file_checksum::calculate_sha256_checksum(source).await?),
                            destination: destination.path().display().to_string(),
                            destination_checksum: None,
                            operation: "link".to_string(),
                            user: Some(std::env::var("USER")?),
                            date: chrono::offset::Local::now(),
                        })
                        .await
                        .map_err(|e| e.into_anyhow())?;

                    info!(
                        "Link: '{}' -> '{}'",
                        source.display(),
                        destination.path().display()
                    );
                }
            }
            FileOperation::Create {
                content,
                destination,
                owner,
                group,
                permissions,
                template,
            } => {
                let store = match destination {
                    Destination::Home(_) => &stores.0,
                    Destination::Root(_) => {
                        stores.1.as_ref().expect("System store should not be empty")
                    }
                };
                // Perform create operation
                debug!(
                    "Trying to create {:?} with specified content",
                    destination.path()
                );

                if !store
                    .check_backup_exists(destination.path())
                    .await
                    .map_err(|e| e.into_anyhow())?
                    & file_fs::check_file_exists(destination.path()).await?
                {
                    store
                        .add_backup(destination.path())
                        .await
                        .map_err(|e| e.into_anyhow())?;
                }

                destination
                    .create(content, *template, context, hb)
                    .await?;

                file_metadata::set_file_metadata(
                    destination.path(),
                    file_metadata::FileMetadata {
                        uid: owner.as_ref().map(file_permissions::user_to_uid).transpose()?,
                        gid: group.as_ref().map(file_permissions::group_to_gid).transpose()?,
                        permissions: permissions
                            .as_ref()
                            .map(file_permissions::perms_str_to_int)
                            .transpose()?,
                        is_symlink: false,
                        symlink_source: None,
                        checksum: None,
                    },
                )
                .await?;

                store
                    .add_file(crate::store::files::StoreFile {
                        module: self.module.clone(),
                        source: None,
                        source_checksum: None,
                        destination: destination.path().display().to_string(),
                        destination_checksum: Some(
                            file_checksum::calculate_sha256_checksum(destination.path()).await?,
                        ),
                        operation: "create".to_string(),
                        user: Some(std::env::var("USER")?),
                        date: chrono::offset::Local::now(),
                    })
                    .await
                    .map_err(|e| e.into_anyhow())?;

                info!("Create: '{}'", destination.path().display());
            }
        };
        Ok(())
    }
}

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
    stores: &(crate::store::db::Store, Option<crate::store::db::Store>),
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
            .0
            .get_all_files(&module_name)
            .await
            .map_err(|e| e.into_anyhow())?
            .into_iter()
            .map(|f| (f.destination, (f.source, f.operation)))
            .collect();
        let mut sys_files: HashMap<String, (Option<String>, String)> =
            if let Some(ref sys_store) = stores.1 {
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
                .0
                .remove_file(&k)
                .await
                .map_err(|e| e.into_anyhow())?;

            file_fs::delete_file(&k)
                .await
                .with_context(|| format!("Failed to remove file {:?}", &k))?;

            // Restore backup
            if stores
                .0
                .check_backup_exists(&k)
                .await
                .map_err(|e| e.into_anyhow())?
            {
                stores
                    .0
                    .restore_backup(&k, &k)
                    .await
                    .map_err(|e| e.into_anyhow())?;
                // TODO validate restored backup
                // Remove backup from store
                stores
                    .0
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
                .1
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
                .1
                .as_ref()
                .unwrap()
                .check_backup_exists(&k)
                .await
                .map_err(|e| e.into_anyhow())?
            {
                stores
                    .1
                    .as_ref()
                    .unwrap()
                    .restore_backup(&k, &k)
                    .await
                    .map_err(|e| e.into_anyhow())?;
                // TODO validate restored backup
                // Remove backup from store
                stores
                    .1
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
