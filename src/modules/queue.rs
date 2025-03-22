use crate::config::DotdeployConfig;
use crate::modules::DeployPhase;
use crate::modules::files::ModuleFile;
use crate::modules::generate_file::Generate;
use crate::modules::messages::CommandMessage;
use crate::modules::tasks::ModuleTask;
use crate::modules::{DotdeployModule, DotdeployModuleBuilder};
use crate::phases::DeployPhaseStruct;
use crate::phases::file::{PhaseFile, PhaseFileOp};
use crate::phases::task::{PhaseHook, PhaseTask};
use crate::utils::file_fs;
use color_eyre::eyre::{OptionExt, WrapErr, eyre};
use color_eyre::{Report, Result, Section};
use handlebars::Handlebars;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::task::JoinSet;
use toml::Value;

use super::packages::InstallPackage;

/// Represents a queue of modules to be processed for deployment.
#[derive(Debug)]
pub(crate) struct ModulesQueue {
    /// A set of modules.
    pub(crate) modules: Vec<DotdeployModule>,
}

impl ModulesQueue {
    /// Collects module names into the context for template processing
    ///
    /// Populates the `DOD_MODULES` key in the context with an array of module names. This array can
    /// be used in handlebars templates to reference other modules.
    ///
    /// * `context` - Mutable reference to template context being built
    pub(crate) fn collect_module_names(&self, context: &mut HashMap<String, Value>) -> Result<()> {
        let mut names = vec![];
        for module in self.modules.iter() {
            names.push(Value::String(module.name.clone()));
        }
        context.insert("DOD_MODULES".to_string(), Value::Array(names));
        Ok(())
    }

    /// Merges module-specific context variables into the global context
    ///
    /// * `context` - Mutable reference to global template context
    pub(crate) fn collect_context(&mut self, context: &mut HashMap<String, Value>) -> Result<()> {
        for module in self.modules.iter_mut() {
            let mod_context = module.context_vars.take();
            if let Some(mod_context) = mod_context {
                context.extend(mod_context);
            }
        }
        Ok(())
    }

    /// Finalizes module configurations by validating and evaluating conditions
    ///
    /// Performs two crucial steps for each module:
    /// 1. Validates module configuration integrity
    /// 2. Evaluates conditional expressions against the context
    ///
    /// * `context` - Template context containing variables for condition evaluation
    /// * `hb` - Handlebars instance for template processing
    ///
    /// # Errors
    /// * Returns error if any module fails validation
    /// * Returns error if condition evaluation fails for any module element
    pub(crate) fn finalize<T>(&mut self, context: &T, hb: &Handlebars<'static>) -> Result<()>
    where
        T: Serialize,
    {
        // Process each module in sequence
        for module in self.modules.iter_mut() {
            // Validate module configuration integrity
            module
                .validate()
                .wrap_err_with(|| format!("Failed to validate module {}", &module.name))?;

            // Evaluate conditions for files, tasks and messages
            module.eval_conditions(&context, hb).wrap_err_with(|| {
                format!("Failed to evaluate conditionals in module {}", &module.name)
            })?;
        }

        Ok(())
    }

    /// Processes all modules to generate deployment phase structures
    ///
    /// Transforms module configurations into executable deployment phases:
    /// - Expands file paths and wildcards
    /// - Organizes files into setup/config phases
    /// - Handles template configuration for files
    ///
    /// # Returns
    /// Tuple containing three deployment phases: (setup, deploy, config)
    ///
    /// # Errors
    /// * Returns error for invalid wildcard usage
    /// * Returns error for missing template configuration
    /// * Returns error for invalid phase specification
    pub(crate) async fn process(
        &mut self,
        config: Arc<DotdeployConfig>,
    ) -> Result<(
        DeployPhaseStruct,
        DeployPhaseStruct,
        Vec<InstallPackage>,
        Vec<Generate>,
        Vec<CommandMessage>,
    )> {
        // Initialize phase containers for each deployment stage
        // - setup: preparation tasks before deployment
        // - config: post-deployment configuration
        let setup_phase = Arc::new(Mutex::new(DeployPhaseStruct::default()));
        let config_phase = Arc::new(Mutex::new(DeployPhaseStruct::default()));

        // Initialize messages container
        let messages = Arc::new(Mutex::new(Vec::new()));
        // Initialize file generator container
        let file_generators = Arc::new(Mutex::new(Vec::new()));
        // Initialize packages container
        let packages = Arc::new(Mutex::new(Vec::new()));

        let seen_files = Arc::new(Mutex::new(HashSet::<String>::new()));

        let mut set: JoinSet<Result<(), Report>> = JoinSet::new();

        while let Some(mut module) = self.modules.pop() {
            let setup_phase = Arc::clone(&setup_phase);
            let config_phase = Arc::clone(&config_phase);
            let seen_files = Arc::clone(&seen_files);
            let config = Arc::clone(&config);
            let messages = Arc::clone(&messages);
            let file_generators = Arc::clone(&file_generators);
            let packages = Arc::clone(&packages);

            set.spawn(async move {
                if let Some(files) = module.files.take() {
                    // Process files based on their phase
                    Self::process_files(
                        files,
                        &mut module,
                        &setup_phase,
                        &config_phase,
                        &config,
                        &seen_files,
                    )
                    .await?;
                };

                if let Some(tasks) = module.tasks.take() {
                    Self::process_tasks(tasks, &mut module, &setup_phase, &config_phase).await?;
                };

                if let Some(module_messages) = module.messages.take() {
                    messages
                        .lock()
                        .map_err(|e| eyre!("Failed to acquire lock {:?}", e))?
                        .append(
                            &mut module_messages
                                .into_iter()
                                .map(|m| CommandMessage {
                                    module_name: module.name.clone(),
                                    message: m.message,
                                    on_command: m.on_command,
                                })
                                .collect(),
                        );
                }

                if let Some(file_gens) = module.generators.take() {
                    // FIXME 2025-03-20: This should be possible to solve more elegantly.
                    let mut new_fgens = Vec::new();
                    for fg in file_gens.into_iter() {
                        let mut target = fg.target;
                        target = Self::expand_target_path(&target, &module).await?;
                        new_fgens.push(Generate {
                            target,
                            source: fg.source,
                            shebang: fg.shebang,
                            comment_start: fg.comment_start,
                            prepend: fg.prepend,
                            append: fg.append,
                            condition: fg.condition,
                        })
                    }
                    file_generators
                        .lock()
                        .map_err(|e| eyre!("Failed to acquire lock {:?}", e))?
                        .append(&mut new_fgens)
                }

                if let Some(module_packages) = module.packages.take() {
                    dbg!(&module_packages);
                    for pkgs in module_packages.into_iter() {
                        // FIXME 2025-03-21: Now we have to handle an empty string in the packages
                        if pkgs.install.is_empty() {
                            packages
                                .lock()
                                .map_err(|e| eyre!("Failed to acquire lock {:?}", e))?
                                .push(InstallPackage {
                                    module_name: module.name.clone(),
                                    package: "".to_string(),
                                })
                        } else {
                            for pkg in pkgs.install {
                                packages
                                    .lock()
                                    .map_err(|e| eyre!("Failed to acquire lock {:?}", e))?
                                    .push(InstallPackage {
                                        module_name: module.name.clone(),
                                        package: pkg,
                                    });
                            }
                        }
                    }
                }
                Ok(())
            });
        }

        // Wait for all operations to complete
        let results = set.join_all().await.into_iter().collect::<Vec<_>>();
        if results.iter().any(|r| r.is_err()) {
            // Collect and combine errors
            let err = results
                .into_iter()
                .filter(Result::is_err)
                .map(Result::unwrap_err)
                .fold(eyre!("Failed to process modules"), |report, e| {
                    report.with_error(|| crate::errors::StrError(format!("{:?}", e)))
                });

            return Err(err);
        }

        Ok((
            Arc::try_unwrap(setup_phase)
                .map_err(|e| eyre!("Failed to unwrap Arc {:?}", e))?
                .into_inner()?,
            Arc::try_unwrap(config_phase)
                .map_err(|e| eyre!("Failed to unwrap Arc {:?}", e))?
                .into_inner()?,
            Arc::try_unwrap(packages)
                .map_err(|e| eyre!("Failed to unwrap Arc {:?}", e))?
                .into_inner()?,
            Arc::try_unwrap(file_generators)
                .map_err(|e| eyre!("Failed to unwrap Arc {:?}", e))?
                .into_inner()?,
            Arc::try_unwrap(messages)
                .map_err(|e| eyre!("Failed to unwrap Arc {:?}", e))?
                .into_inner()?,
        ))
    }

    /// Processes module files and distributes them to appropriate deployment phases
    ///
    /// This function:
    /// - Expands file paths with module context
    /// - Handles wildcard patterns in source/target paths
    /// - Creates phase-specific file entries with proper metadata
    /// - Distributes files to setup/deploy/config phases based on configuration
    ///
    /// # Arguments
    /// * `files` - Vector of module files to process
    /// * `module` - Module these files belong to
    /// * `setup_phase` - Setup phase container to populate
    /// * `deploy_phase` - Deploy phase container to populate
    /// * `config_phase` - Config phase container to populate
    ///
    /// # Errors
    /// * Returns error if path expansion fails
    /// * Returns error if wildcard expansion fails
    /// * Returns error if required fields are missing
    /// * Returns error if invalid phase is specified
    async fn process_files(
        files: Vec<ModuleFile>,
        module: &mut DotdeployModule,
        setup_phase: &Arc<Mutex<DeployPhaseStruct>>,
        config_phase: &Arc<Mutex<DeployPhaseStruct>>,
        config: &Arc<DotdeployConfig>,
        seen_files: &Arc<Mutex<HashSet<String>>>,
    ) -> Result<()> {
        let mut phase_files = vec![];

        // Destructure file
        for file in files {
            let ModuleFile {
                mut source,
                mut target,
                content,
                phase,
                operation,
                condition: _,
                template,
                owner,
                group,
                permissions,
            } = file;

            // Expand source file names
            if let Some(ref mut source) = source {
                *source = Self::expand_source_path(source, module)
                    .await
                    .wrap_err_with(|| {
                        format!(
                            "Failed to expand source path in module={} for file={}",
                            &module.name, &source
                        )
                    })?;
            }

            // Expand target file names
            target = Self::expand_target_path(&target, module)
                .await
                .wrap_err_with(|| {
                    format!(
                        "Failed to expand target path in module={} for file={}",
                        &module.name, &target
                    )
                })?;

            // Check that if target is outside of user's HOME directory, deploy_sys_files is true
            if target.starts_with(&file_fs::path_to_string(
                dirs::home_dir().ok_or_eyre("Failed to get user's HOME dir")?,
            )?) && !&config.deploy_sys_files
            {
                return Err(eyre!(
                    "{} is outside of your HOME directory but this feature is currently disabled",
                    &target
                )
                .suggestion("Check the value of 'deploy_sys_files' in the dotdeploy config"));
            }

            if let Some(expanded_pairs) = Self::handle_wildcard_expansion(&source, &target)
                .await
                .wrap_err_with(|| {
                    format!("Failed to expand wildcards in module={}", &module.name)
                })?
            {
                // Create PhaseFile for each expanded pair
                for (expanded_source, expanded_target) in expanded_pairs {
                    if !seen_files
                        .lock()
                        .map_err(|e| eyre!("Failed to acquire lock {:?}", e))?
                        .insert(expanded_target.clone())
                    {
                        return Err(eyre!("{} declared multiple times", &expanded_target));
                    }

                    phase_files.push(
                        Self::build_phase_files(
                            &Some(expanded_source),
                            &expanded_target,
                            module,
                            content.clone(),
                            operation.clone(),
                            template,
                            owner.clone(),
                            group.clone(),
                            permissions.clone(),
                        )
                        .await
                        .wrap_err_with(|| {
                            format!("Failed to build PhaseFile for file={}", &expanded_target)
                        })?,
                    );
                }
            } else {
                if !seen_files
                    .lock()
                    .map_err(|e| eyre!("Failed to acquire lock {:?}", e))?
                    .insert(target.clone())
                {
                    return Err(eyre!("{} declared multiple times", &target));
                }

                phase_files.push(
                    Self::build_phase_files(
                        &source,
                        &target,
                        module,
                        content,
                        operation,
                        template,
                        owner,
                        group,
                        permissions,
                    )
                    .await
                    .wrap_err_with(|| format!("Failed to build PhaseFile for file={}", &target))?,
                );
            }
            dbg!(&phase, &phase_files);
            match phase {
                DeployPhase::Setup => setup_phase
                    .lock()
                    .map_err(|e| eyre!("Failed to acquire lock {:?}", e))?
                    .files
                    .append(&mut phase_files),
                DeployPhase::Config => config_phase
                    .lock()
                    .map_err(|e| eyre!("Failed to acquire lock {:?}", e))?
                    .files
                    .append(&mut phase_files),
                other => {
                    return Err(eyre!("Invalid phase specified: {:?}", other));
                }
            }
        }
        Ok(())
    }

    /// Handles source path expansion with module context
    ///
    /// Expands environment variables in source paths and resolves relative paths against the
    /// current module's location.
    ///
    /// # Arguments
    /// * `source` - Source path string, possibly containing env vars
    /// * `module` - Module providing location context
    ///
    /// # Returns
    /// Fully expanded absolute path as a string
    ///
    /// # Errors
    /// * Returns error if path expansion fails
    async fn expand_source_path(source: &str, module: &DotdeployModule) -> Result<String> {
        // Make the current module location available as an env var
        let mut env = HashMap::new();
        env.insert(
            "DOD_CURRENT_MODULE".to_string(),
            file_fs::path_to_string(module.location.clone())?,
        );

        // Expand env vars in path
        let expanded = file_fs::expand_path_string(source, Some(&env))?;
        // If the path start with '/' we assume it is absolute
        if expanded.starts_with('/') {
            Ok(expanded)
        } else {
            // Otherwise, expand it relative to the current module directory
            file_fs::expand_path_string(
                &format!(
                    "$DOD_CURRENT_MODULE{}{}",
                    std::path::MAIN_SEPARATOR_STR,
                    &expanded
                ),
                Some(&env),
            )
        }
    }

    /// Handles target path expansion and validation
    ///
    /// Expands environment variables in target paths and ensures they are absolute. Relative paths
    /// are rejected as invalid targets.
    ///
    /// # Arguments
    /// * `target` - Target path string, possibly containing env vars
    /// * `module` - Module providing location context
    ///
    /// # Returns
    /// Fully expanded absolute path as a string
    ///
    /// # Errors
    /// * Returns error if path expansion fails
    /// * Returns error if target path is not absolute
    async fn expand_target_path(target: &str, module: &DotdeployModule) -> Result<String> {
        // Make the current module location available as an env var
        let mut env = HashMap::new();
        env.insert(
            "DOD_CURRENT_MODULE".to_string(),
            file_fs::path_to_string(module.location.clone())?,
        );

        let expanded = file_fs::expand_path_string(target, Some(&env))?.replace("##dot##", ".");

        if expanded.starts_with('/') {
            Ok(expanded)
        } else {
            Err(eyre!(
                "Invalid target file name: {} -> {}",
                target,
                expanded
            ))
        }
    }

    /// Handles wildcard expansion for source and target paths
    ///
    /// Determines if wildcard expansion is needed and validates that both source and target use
    /// wildcards consistently.
    ///
    /// # Arguments
    /// * `source` - Optional source path that may contain wildcards
    /// * `target` - Target path that may contain wildcards
    ///
    /// # Returns
    ///
    /// * `Some(Vec<(String, String)>)` - List of expanded (source, target) pairs if wildcards
    ///   present
    /// * `None` - If no wildcards are present
    ///
    /// # Errors
    /// * Returns error if only one path has a wildcard
    /// * Returns error if target has wildcard but source is None
    async fn handle_wildcard_expansion(
        source: &Option<String>,
        target: &str,
    ) -> Result<Option<Vec<(String, String)>>> {
        match (source.as_ref(), target) {
            (Some(src), tgt) if src.ends_with('*') && tgt.ends_with('*') => {
                Ok(Some(expand_wildcards(src, tgt)?))
            }
            (Some(src), tgt) if src.ends_with('*') || tgt.ends_with('*') => Err(eyre!(
                "Both source and target must end with '*' for wildcard expansion: source={}, target={}",
                src,
                tgt
            )),
            (None, tgt) if tgt.ends_with('*') => Err(eyre!(
                "Target '{}' has wildcard but source is not specified",
                tgt
            )),
            _ => Ok(None),
        }
    }

    /// Builds a PhaseFile structure from file configuration
    ///
    /// Creates a deployment phase file entry with all necessary metadata for file operations during
    /// deployment.
    ///
    /// # Arguments
    /// * `source` - Optional source path for the file
    /// * `target` - Target path where file will be deployed
    /// * `module` - Module this file belongs to
    /// * `content` - Optional inline content for the file
    /// * `operation` - File operation type (copy/link/create)
    /// * `template` - Whether file should be processed as a template
    /// * `owner` - Optional file ownership specification
    /// * `group` - Optional file group specification
    /// * `permissions` - Optional file permissions specification
    ///
    /// # Errors
    /// * Returns error if required fields are missing
    async fn build_phase_files(
        source: &Option<String>,
        target: &str,
        module: &DotdeployModule,
        content: Option<String>,
        operation: Option<String>,
        template: Option<bool>,
        owner: Option<String>,
        group: Option<String>,
        permissions: Option<String>,
    ) -> Result<PhaseFile> {
        Ok(PhaseFile {
            module_name: module.name.clone(),
            source: source.as_ref().map(PathBuf::from),
            target: PathBuf::from(target),
            content,
            operation: match operation.as_deref() {
                Some("copy") => PhaseFileOp::Copy,
                Some("link") => PhaseFileOp::Link,
                Some("create") => PhaseFileOp::Create,
                _ => unreachable!(),
            },
            template: template.ok_or_eyre(format!(
                "Template field required for expanded file={} in module={}",
                &target, &module.name
            ))?,
            owner,
            group,
            permissions,
        })
    }

    /// Processes module tasks and distributes them to appropriate deployment phases
    ///
    /// This function:
    /// - Expands paths in task commands
    /// - Creates phase-specific task entries with proper metadata
    /// - Distributes tasks to setup/deploy/config phases based on configuration
    ///
    /// # Arguments
    /// * `tasks` - Vector of module tasks to process
    /// * `module` - Module these tasks belong to
    /// * `setup_phase` - Setup phase container to populate
    /// * `deploy_phase` - Deploy phase container to populate
    /// * `config_phase` - Config phase container to populate
    ///
    /// # Errors
    /// * Returns error if path expansion fails
    /// * Returns error if required fields are missing
    /// * Returns error if invalid phase is specified
    async fn process_tasks(
        tasks: Vec<ModuleTask>,
        module: &mut DotdeployModule,
        setup_phase: &Arc<Mutex<DeployPhaseStruct>>,
        config_phase: &Arc<Mutex<DeployPhaseStruct>>,
    ) -> Result<()> {
        for task in tasks {
            let ModuleTask {
                shell,
                exec,
                args,
                sudo,
                phase,
                hook,
                condition: _,
            } = task;

            let phase_task = PhaseTask {
                module_name: module.name.clone(),
                shell,
                exec: exec
                    .map(|x| {
                        let mut env = HashMap::new();
                        env.insert(
                            "DOD_CURRENT_MODULE".to_string(),
                            file_fs::path_to_string(module.location.clone())?,
                        );
                        file_fs::expand_path_string(&x, Some(&env))
                            .map_err(|e| eyre!("Failed to expand path '{}': {:?}", x, e))
                    })
                    .transpose()?,
                args,
                sudo: sudo.ok_or_eyre("sudo field required")?,
                hook: match hook.as_deref() {
                    Some("pre") => PhaseHook::Pre,
                    Some("post") => PhaseHook::Post,
                    _ => unreachable!(),
                },
            };

            match phase {
                DeployPhase::Setup => setup_phase
                    .lock()
                    .map_err(|e| eyre!("Failed to acquire lock {:?}", e))?
                    .tasks
                    .push(phase_task),
                DeployPhase::Config => config_phase
                    .lock()
                    .map_err(|e| eyre!("Failed to acquire lock {:?}", e))?
                    .tasks
                    .push(phase_task),
                other => return Err(eyre!("Invalid phase specified: {:?}", other)),
            }
        }
        Ok(())
    }
}

/// Builder for [`ModulesQueue`] by resolving module dependencies and processing
/// module configurations.
#[derive(Debug, Default)]
pub(crate) struct ModulesQueueBuilder {
    /// Modules to be processed (in raw, unprocessed form)
    pub(crate) modules: Option<Vec<String>>,
}

impl ModulesQueueBuilder {
    /// Creates a new ModulesQueueBuilder with default settings.
    pub(crate) fn new() -> Self {
        ModulesQueueBuilder::default()
    }

    /// Adds modules to the queue.
    ///
    /// # Arguments
    /// * `modules` - List of module names to process
    pub(crate) fn with_modules(&mut self, modules: Vec<String>) -> &mut Self {
        let new = self;
        new.modules = Some(modules);
        new
    }

    /// Constructs the ModulesQueue instance by processing all modules
    /// and their dependencies in the correct order.
    ///
    /// # Errors
    /// Returns an error if any module cannot be loaded or if dependency
    /// resolution fails.
    pub(crate) fn build(&mut self, dotdeploy_config: &DotdeployConfig) -> Result<ModulesQueue> {
        match self.modules {
            Some(ref modules) => {
                let mut processed = HashMap::new();
                let mut mod_queue = vec![];

                for module in modules {
                    // These modules have been requested by the user and should be "manual"
                    Self::process_module(
                        module,
                        dotdeploy_config,
                        &mut processed,
                        &mut mod_queue,
                        Some("manual".to_string()),
                    )?;
                }

                Ok(ModulesQueue { modules: mod_queue })
            }
            None => todo!(),
        }
    }

    /// Recursively processes modules and their dependencies
    ///
    /// # Arguments
    /// * `module_name` - Name of module to process
    /// * `dotdeploy_config` - Application configuration containing paths
    /// * `processed` - HashMap tracking already processed modules and their reasons
    /// * `mod_queue` - Output vector for ordered modules
    /// * `reason` - Installation reason; "manual" or "automatic" (dependencies)
    ///
    /// # Errors
    /// Returns an error if module configuration cannot be loaded or built
    fn process_module(
        module_name: &str,
        dotdeploy_config: &DotdeployConfig,
        processed: &mut HashMap<String, String>,
        mod_queue: &mut Vec<DotdeployModule>,
        reason: Option<String>,
    ) -> Result<()> {
        let reason = reason.or_else(|| Some("automatic".to_string()));

        if let Some(existing_reason) = processed.get(module_name) {
            match existing_reason.as_str() {
                // Skip processing if already exists with manual reason
                "manual" => return Ok(()),
                // Skip processing if already exists and both reasons are automatic
                "automatic" if reason.as_deref() == Some("automatic") => return Ok(()),
                // Otherwise continue
                _ => (),
            }
        }

        // Remove any existing automatic entries from the queue
        if reason.as_deref() == Some("manual") {
            mod_queue.retain(|m| m.name != module_name);
        }

        // Build module configuration with correct reason
        let reason_str = reason.unwrap_or_else(|| "automatic".to_string());
        let mod_conf = DotdeployModuleBuilder::from_toml(module_name, dotdeploy_config)?
            .build(reason_str.clone())?;

        // Update tracking with actual reason used
        processed.insert(module_name.to_string(), reason_str);

        // Process dependencies first (depth-first)
        if let Some(dependencies) = &mod_conf.depends_on {
            for dep in dependencies {
                // Dependencies should always be "automatic"
                Self::process_module(dep, dotdeploy_config, processed, mod_queue, None)?;
            }
        }

        // Add module to queue after its dependencies
        mod_queue.push(mod_conf);

        Ok(())
    }
}

/// Expands wildcard patterns in source and target paths into concrete file pairs. Both paths must
/// end with a wildcard ('*') to be eligible for expansion.
///
/// # Arguments
/// * `source` - Source path with wildcard ending
/// * `target` - Target path with wildcard ending
///
/// # Returns
/// A vector of (expanded_source, expanded_target) pairs
///
/// # Errors
/// * Returns error if paths don't end with '*'
/// * Returns error if source directory cannot be read
/// * Returns error if no files are found in source directory
/// * Returns error if file paths contain invalid UTF-8
fn expand_wildcards(source: &str, target: &str) -> Result<Vec<(String, String)>> {
    // Validate both paths end with '*'
    if !source.ends_with('*') || !target.ends_with('*') {
        return Err(eyre!(
            "Both source and target must end with '*' for wildcard expansion"
        ));
    }

    // Get the parent directories by removing the wildcard
    let source_parent = source.trim_end_matches('*').trim_end_matches('/');
    let target_parent = target.trim_end_matches('*').trim_end_matches('/');

    // Read the source directory
    let source_dir = Path::new(source_parent);
    let entries = std::fs::read_dir(source_dir)
        .wrap_err_with(|| format!("Failed to read source directory: {}", source_dir.display()))?;

    // Create expanded pairs
    let mut expanded = Vec::new();
    for entry in entries {
        let entry = entry.wrap_err("Failed to read directory entry")?;
        let entry_path = entry.path();

        // Skip directories if needed
        if entry_path.is_dir() {
            continue; // Optionally skip directories
        }

        let file_name = entry_path
            .file_name()
            .ok_or_eyre("Failed to get file name")?
            .to_str()
            .ok_or_eyre("File name contains invalid UTF-8")?;

        let expanded_source = entry_path
            .to_str()
            .ok_or_eyre("Path contains invalid UTF-8")?
            .to_string();

        let expanded_target = format!("{}/{}", target_parent, file_name);
        expanded.push((expanded_source, expanded_target));
    }

    if expanded.is_empty() {
        return Err(eyre!(
            "No files found in source directory: {}",
            source_parent
        ));
    }

    Ok(expanded)
}

// -------------------------------------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn create_test_module(name: &str) -> DotdeployModule {
        DotdeployModule {
            name: name.to_string(),
            location: PathBuf::from("/dummy/path"),
            reason: "automatic".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_collect_module_names() -> Result<()> {
        // Empty queue
        let queue = ModulesQueue { modules: vec![] };
        let mut context = HashMap::new();
        queue.collect_module_names(&mut context)?;

        let names = context
            .get("DOD_MODULES")
            .ok_or_eyre("DOD_MODULES key missing")?
            .as_array()
            .ok_or_eyre("DOD_MODULES value not array")?;

        assert!(names.is_empty(), "Should have empty array with no modules");

        // Single entry in queue
        let module = create_test_module("test_module");
        let queue = ModulesQueue {
            modules: vec![module],
        };
        let mut context = HashMap::new();
        queue.collect_module_names(&mut context)?;

        let names = context["DOD_MODULES"]
            .as_array()
            .ok_or_eyre("DOD_MODULES value not array")?;

        assert_eq!(names.len(), 1, "Should collect single module name");
        assert_eq!(
            names[0].as_str().unwrap(),
            "test_module",
            "Module name should match"
        );

        // Multiple entry in queue
        let modules = vec![
            create_test_module("module1"),
            create_test_module("module2"),
            create_test_module("module3"),
        ];
        let queue = ModulesQueue { modules };
        let mut context = HashMap::new();
        queue.collect_module_names(&mut context)?;

        let names = context["DOD_MODULES"]
            .as_array()
            .ok_or_eyre("DOD_MODULES value not array")?;

        assert_eq!(names.len(), 3, "Should collect all module names");
        assert_eq!(
            names
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["module1", "module2", "module3"],
            "Module names should be in order"
        );

        Ok(())
    }

    #[test]
    fn test_expand_wildcards() -> Result<()> {
        // Test basic wildcard expansion
        let dir = tempdir()?;
        let file1 = dir.path().join("file1.txt");
        std::fs::write(&file1, b"")?;
        let file2 = dir.path().join("file2.txt");
        std::fs::write(&file2, b"")?;

        let source = format!("{}/*", dir.path().to_str().unwrap());
        let target = "/dest/*";
        let pairs = expand_wildcards(&source, target)?;

        let mut expected = vec![
            (
                file1.to_str().unwrap().to_string(),
                "/dest/file1.txt".to_string(),
            ),
            (
                file2.to_str().unwrap().to_string(),
                "/dest/file2.txt".to_string(),
            ),
        ];
        expected.sort();

        let mut sorted_pairs = pairs;
        sorted_pairs.sort();
        assert_eq!(sorted_pairs, expected);

        // Test missing wildcard in source
        let result = expand_wildcards("/valid/path", "/dest/*");
        assert!(result.is_err(), "Should error when source lacks wildcard");

        // Test missing wildcard in target
        let result = expand_wildcards("/src/*", "/dest");
        assert!(result.is_err(), "Should error when target lacks wildcard");

        // Test empty directory
        let empty_dir = tempdir()?;
        let empty_source = format!("{}/*", empty_dir.path().to_str().unwrap());
        let result = expand_wildcards(&empty_source, "/dest/*");
        assert!(result.is_err(), "Should error on empty directory");

        // Test directory with subdirectory
        let sub_dir = dir.path().join("subdir");
        std::fs::create_dir(&sub_dir)?;
        let result = expand_wildcards(&source, target)?;
        assert_eq!(result.len(), 2, "Should ignore directories");

        // Test non-existent directory
        let result = expand_wildcards("/non/existent/*", "/dest/*");
        assert!(result.is_err(), "Should error on non-existent directory");

        // Test mixed wildcard positions
        let result = expand_wildcards("/src/*.txt", "/dest/*.bak");
        assert!(result.is_err(), "Should require exact wildcard position");

        // Test UTF-8 paths
        let dir = tempdir()?;
        let file = dir.path().join("ñáéíóú.txt");
        std::fs::write(&file, b"")?;
        let source = format!("{}/*", dir.path().to_str().unwrap());
        let pairs = expand_wildcards(&source, "/dest/*")?;
        assert!(!pairs.is_empty(), "Should handle UTF-8 filenames");

        Ok(())
    }
}
