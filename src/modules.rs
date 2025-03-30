use crate::config::DotdeployConfig;
use crate::modules::files::FileOperation;
use crate::modules::files::ModuleFile;
use crate::modules::generate_file::Generate;
use crate::modules::messages::ModuleMessage;
use crate::modules::tasks::ModuleTask;
use crate::utils::file_fs;
use color_eyre::eyre::{OptionExt, WrapErr, eyre};
use color_eyre::{Result, Section};
use handlebars::Handlebars;
use packages::ModulePackages;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use toml::Value;

pub(crate) mod files;
mod generate_file;
pub(crate) mod messages;
mod packages;
pub(crate) mod queue;
pub(crate) mod tasks;

#[derive(Debug, Default)]
pub(crate) struct DotdeployModule {
    /// Module name
    pub(crate) name: String,
    /// Module location/path
    pub(crate) location: PathBuf,
    /// The reason for adding this module (e.g., "manual" or "automatic")
    pub(crate) reason: String,
    /// Module dependencies
    pub(crate) depends_on: Option<Vec<String>>,
    /// Other config TOML files to include
    pub(crate) includes: Option<Vec<PathBuf>>,
    /// Files to be managed by this module
    pub(crate) files: Option<Vec<ModuleFile>>,
    /// Tasks to be executed by this module
    pub(crate) tasks: Option<Vec<ModuleTask>>,
    /// Messages to be displayed during module execution
    pub(crate) messages: Option<Vec<ModuleMessage>>,
    /// Messages to be displayed during module execution
    pub(crate) generators: Option<Vec<Generate>>,
    /// Packages to install for this module
    pub(crate) packages: Option<Vec<ModulePackages>>,
    /// Key-value pairs used for handlebars templating
    pub(crate) context_vars: Option<HashMap<String, Value>>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
pub(crate) enum DeployPhase {
    Setup,
    #[default]
    Config,
    Update,
    Remove,
}

/// Filters and retains components based on condition evaluation
///
/// Internal helper macro that processes a module's component collections (files, tasks, etc.),
/// retaining only elements whose conditions evaluate successfully to `true`. Handles error logging
/// consistently while maintaining component-specific context details.
macro_rules! retain_components {
    ($self:ident, $field:ident, $context:ident, $hb:ident) => {
        if let Some(ref mut components) = $self.$field {
            components.retain(|component| {
                component
                    .eval_condition($context, $hb)
                    .unwrap_or_else(|err| {
                        component.log_error(&$self.name, &$self.location, err);
                        false
                    })
            });
        }
    };
}

/// Defines error logging contract for conditionally evaluated components
///
/// Implemented by all component types (files, tasks, etc.) to provide consistent error reporting
/// while preserving component-specific context in logs.
trait ConditionalComponent {
    /// Records condition evaluation failures with component-specific metadata
    ///
    /// * `module_name` - Parent module where error occurred  
    /// * `location` - Module configuration path
    /// * `err` - Error details from evaluation failure
    fn log_error(&self, module_name: &str, location: &Path, err: impl std::fmt::Display);
}

impl DotdeployModule {
    /// Validates the module configuration for consistency and correctness.
    ///
    /// This function checks:
    /// - Files cannot have both `source` and `content` defined
    /// - Templating is only allowed for 'create' or 'copy' operations
    /// - Tasks cannot have both `shell` and `exec` commands defined
    ///
    /// # Errors
    ///
    /// Returns an error if any validation rule is violated, with details about
    /// the specific issue.
    pub(crate) fn validate(&self) -> Result<()> {
        // Validate files
        if let Some(ref files) = self.files {
            for file in files.iter() {
                // Check if both source and content are defined
                if file.source.is_some() & file.content.is_some() {
                    return Err(eyre!(
                        "{}:{}\n A file can have either source OR content defined",
                        &self.location.display(),
                        &file.target.display()
                    ));
                }

                // Check if template is used only for files with type "copy" or "create"
                if file.template.is_some_and(|x| x)
                    & match file.operation {
                        FileOperation::Copy => false,
                        FileOperation::Link => true,
                        FileOperation::Create => false,
                    }
                {
                    return Err(eyre!(
                        "{}:{}\n Templating is only allowed for operations of type 'create' or 'copy'",
                        &self.location.display(),
                        &file.target.display()
                    ));
                }
            }
        }

        // Validate tasks
        if let Some(ref tasks) = self.tasks {
            for task in tasks.iter() {
                if task.shell.is_some() & task.exec.is_some() {
                    let command_display = match (&task.shell, &task.exec) {
                        (Some(shell), _) => shell,
                        (_, Some(exec)) => exec,
                        _ => &OsString::from("<unknown>"), // This shouldn't happen due to the condition above
                    };

                    return Err(eyre!(
                        "{}:command={}\n A task can be either a shell command OR an executable",
                        &self.location.display(),
                        command_display.to_string_lossy()
                    ));
                }
            }
        }
        Ok(())
    }

    /// Evaluates conditional expressions for all module components and filters active ones
    ///
    /// Processes files, tasks, messages, generators, and packages to retain only components whose
    /// `condition` templates evaluate to true. Errors during evaluation are logged but don't block
    /// execution of other components.
    ///
    /// * `context` - Runtime variables and system information for template evaluation
    /// * `hb` - Shared Handlebars registry with registered helpers
    ///
    /// # Errors
    ///
    /// Returns `Ok(())` even if some components fail evaluation (errors are logged). Only returns
    /// an error if there's a fundamental system failure (unlikely here).
    pub(crate) fn eval_conditions<T>(&mut self, context: &T, hb: &Handlebars<'static>) -> Result<()>
    where
        T: Serialize,
    {
        // Process files with conditional filtering
        retain_components!(self, files, context, hb);
        // Filter tasks based on their execution conditions
        retain_components!(self, tasks, context, hb);
        // Filter messages based on display conditions
        retain_components!(self, messages, context, hb);
        // Process generated file conditions
        retain_components!(self, generators, context, hb);
        // Filter packages based on installation conditions
        retain_components!(self, packages, context, hb);

        Ok(())
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DotdeployModuleBuilder {
    __name__: Option<String>,
    __location__: Option<PathBuf>,
    __reason__: Option<PathBuf>,
    depends_on: Option<Vec<String>>,
    includes: Option<Vec<PathBuf>>,
    files: Option<Vec<ModuleFile>>,
    tasks: Option<Vec<ModuleTask>>,
    messages: Option<Vec<ModuleMessage>>,
    generators: Option<Vec<Generate>>,
    #[serde(default = "default_module_packages")]
    packages: Option<Vec<ModulePackages>>,
    context_vars: Option<HashMap<String, Value>>,
}

fn default_module_packages() -> Option<Vec<ModulePackages>> {
    Some(vec![ModulePackages {
        install: vec![],
        ..Default::default()
    }])
}

impl DotdeployModuleBuilder {
    /// Creates a new DotdeployModuleBuilder from TOML configuration.
    pub(crate) fn from_toml(module_name: &str, dotdeploy_config: &DotdeployConfig) -> Result<Self> {
        let path = locate_module(module_name, dotdeploy_config)
            .wrap_err_with(|| format!("Failed to locate module config file: {:?}", &module_name))?;
        let toml_string = std::fs::read_to_string(path.join("config.toml"))
            .wrap_err_with(|| format!("Failed to read module config file: {:?}", &path))?;
        let mut config: Self = toml::from_str(&toml_string)
            .wrap_err_with(|| format!("Failed to parse module config from: {}", toml_string))?;
        config.__location__ = Some(path);
        config.__name__ = Some(module_name.into());

        Ok(config)
    }

    /// Constructs a DotdeployModule instance from the builder's configuration
    ///
    /// # Errors
    /// Returns an error if required fields (name/location) are missing
    pub(crate) fn build(&mut self, reason: String) -> Result<DotdeployModule> {
        let mut module = DotdeployModule {
            name: Clone::clone(self.__name__.as_ref().ok_or_eyre("Empty 'name' field")?),
            location: Clone::clone(
                self.__location__
                    .as_ref()
                    .ok_or_eyre("Empty 'location' field")?,
            ),
            reason,
            depends_on: self.depends_on.take(),
            includes: self.includes.take(),
            files: self.files.take(),
            tasks: self.tasks.take(),
            messages: self.messages.take(),
            generators: self.generators.take(),
            packages: self.packages.take(),
            context_vars: self.context_vars.take(),
        };

        if let Some(ref includes) = module.includes {
            for include_path in includes {
                // Make the current module location available as an env var
                let mut env = HashMap::new();
                env.insert("DOD_CURRENT_MODULE".to_string(), &module.location);

                let mut expanded = file_fs::expand_path(include_path, Some(&env))?;

                // If the path start with '/' we assume it is absolute
                if !expanded.starts_with("/") {
                    // Otherwise, expand it relative to the current module directory
                    let mut path = PathBuf::from(&module.location);
                    path.push(expanded);
                    expanded = path
                }

                let include_content = std::fs::read_to_string(&expanded).wrap_err_with(|| {
                    format!("Failed to read include file: {}", expanded.display())
                })?;

                let included_config: DotdeployModuleBuilder = toml::from_str(&include_content)
                    .wrap_err_with(|| {
                        format!(
                            "Failed to parse included config from: {}",
                            include_path.display()
                        )
                    })?;

                // Replace any existing files with same target from included config
                if let Some(files) = included_config.files {
                    if let Some(ref mut module_files) = module.files {
                        // Remove any files that have matching targets in the included files
                        let included_targets =
                            files.iter().map(|f| &f.target).collect::<HashSet<_>>();
                        module_files.retain(|f| !included_targets.contains(&f.target));

                        // Add all files from included config
                        module_files.extend(files);
                    }
                }

                // Merge other fields by extending 
                if let Some(included_tasks) = included_config.tasks {
                    module
                        .tasks
                        .get_or_insert_with(Vec::new)
                        .extend(included_tasks);
                }
                if let Some(included_messages) = included_config.messages {
                    module
                        .messages
                        .get_or_insert_with(Vec::new)
                        .extend(included_messages);
                }
                if let Some(included_generators) = included_config.generators {
                    module
                        .generators
                        .get_or_insert_with(Vec::new)
                        .extend(included_generators);
                }
                if let Some(included_packages) = included_config.packages {
                    module
                        .packages
                        .get_or_insert_with(Vec::new)
                        .extend(included_packages);
                }

                // Merge HashMap by inserting (overwriting duplicates)
                if let Some(included_vars) = included_config.context_vars {
                    module
                        .context_vars
                        .get_or_insert_with(HashMap::new)
                        .extend(included_vars);
                }
            }
        }

        Ok(module)
    }
}

/// Locates the path to a module's configuration directory.
///
/// Determines the appropriate root directory based on the module name:
/// - For host-specific modules (starting with "hosts/"), uses the hosts_root
///   directory
/// - For regular modules, uses the modules_root directory
///
/// # Arguments
/// * `module_name` - Name of the module to locate
/// * `dotdeploy_config` - Configuration containing the root directories
///
/// # Errors
///
/// Returns an error if the module name is invalid or if the configuration paths
/// are not set
pub(crate) fn locate_module(
    module_name: &str,
    dotdeploy_config: &DotdeployConfig,
) -> Result<PathBuf> {
    // Determine the path to the module's configuration file
    let path = if module_name.starts_with("hosts/") {
        // For host-specific modules, use the hosts_root directory
        dotdeploy_config
            .hosts_root
            .join(module_name.trim_start_matches("hosts/"))
    } else {
        // For regular modules, use the modules_root directory
        dotdeploy_config.modules_root.join(module_name)
    };

    Ok(path)
}

/// Evaluate a template condition.
///
/// * `context` - Context for template evaluation
/// * `hb` - Handlebars registry with registered helpers
///
/// # Errors
/// Returns an error if template rendering fails due to:
/// * Invalid Handlebars syntax in condition
/// * Missing context variables required by the template
/// * Type mismatches during template evaluation
trait ConditionEvaluator {
    fn eval_condition<T>(&self, context: &T, hb: &Handlebars<'static>) -> Result<bool>
    where
        T: Serialize;

    /// Evaluate the handlebars template in the `if`/`condition` field.
    ///
    /// Constructs a Handlebars template that will return "true" or "false"
    /// based on the evaluation of the condition. It then renders this template
    /// with the provided context and interprets the result.
    fn eval_condition_helper<T>(
        condition: &str,
        context: &T,
        hb: &Handlebars<'static>,
    ) -> Result<bool>
    where
        T: Serialize,
    {
        // Construct a Handlebars template that will evaluate to "true" or "false"
        let eval_template = format!(
            "{{{{#if {condition}}}}}true{{{{else}}}}false{{{{/if}}}}",
            condition = condition
        );

        // Render the template with the provided context
        let result = hb
            .render_template(&eval_template, context)
            .wrap_err_with(|| format!("Failed to evaluate template: {}", eval_template))
            .suggestion("Ensure that the names of the context variables are correct")?;

        Ok(result == "true")
    }
}

// Serde defaults providers
/// Provides default optional boolean value (false).
fn default_option_bool() -> Option<bool> {
    Some(false)
}

/// Provides default phase step value ("post").
fn default_phase_hook() -> Option<String> {
    Some("post".to_string())
}

/// Provides default message display_when value ("deploy").
fn default_on_command() -> Option<String> {
    Some("deploy".to_string())
}
