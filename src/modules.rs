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
    pub(crate) includes: Option<Vec<Include>>,
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

/// Deployment phases for organizing module components
///
/// Defines the execution order for different types of operations:
/// - `Setup`: Initial environment preparation before deployment
/// - `Config`: Primary configuration deployment (default phase)
/// - `Update`: Post-deployment updates and maintenance
/// - `Remove`: Cleanup operations for module removal
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
pub(crate) enum DeployPhase {
    Setup,
    #[default]
    Config,
    Update,
    Remove,
}

/// Module include definition for merging external configurations
///
/// Supports two inclusion formats:
/// - `Simple`: Direct path to a TOML file
/// - `Conditional`: Map with file list and template condition
///
/// # Examples
/// ```toml
/// includes = [
///     "base_config.toml",
///     { files = ["conditional.toml"], if = "(eq DOD_DISTRIBUTION_NAME 'ubuntu')" }
/// ]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum Include {
    Simple(PathBuf),
    Conditional {
        files: Vec<PathBuf>,
        #[serde(rename = "if")]
        condition: Option<String>,
    },
}

impl ConditionEvaluator for Include {
    fn eval_condition<T>(&self, context: &T, hb: &handlebars::Handlebars<'static>) -> Result<bool>
    where
        T: Serialize,
    {
        match self {
            // Always true for Simple
            Include::Simple(_path_buf) => Ok(true),
            Include::Conditional {
                files: _,
                condition,
            } => {
                if let Some(condition) = condition {
                    Self::eval_condition_helper(condition, context, hb)
                } else {
                    // Just return true if there is no condition
                    Ok(true)
                }
            }
        }
    }
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
    /// Processes includes recursively with condition evaluation and context merging
    ///
    /// 1. Evaluates inclusion conditions using current context
    /// 2. Merges configurations from qualified includes
    /// 3. Handles nested includes through iterative processing
    /// 4. Maintains original include order for deterministic behavior
    ///
    /// # Arguments
    /// * `context` - Mutable template context updated with included variables
    /// * `hb` - Handlebars registry for condition evaluation
    ///
    /// # Errors
    /// Returns error for:
    /// - Invalid include paths
    /// - Template evaluation failures
    /// - Configuration merging conflicts
    pub(crate) fn process_includes(
        &mut self,
        context: &mut HashMap<String, Value>,
        hb: &Handlebars<'static>,
    ) -> Result<()> {
        let mut processed_includes = Vec::new();
        let mut pending_includes = self.includes.take().unwrap_or_default();

        while !pending_includes.is_empty() {
            for include in pending_includes.drain(..) {
                let condition_met = include.eval_condition(context, hb)?;
                if condition_met {
                    match include {
                        Include::Simple(path) => {
                            self.merge_include(&path, context)?;
                            processed_includes.push(Include::Simple(path));
                        }
                        Include::Conditional { files, condition } => {
                            for file in files.iter() {
                                self.merge_include(file, context)?;
                            }
                            processed_includes.push(Include::Conditional { files, condition });
                        }
                    }
                }
            }

            // Get new includes added by merged configurations
            pending_includes = self.includes.take().unwrap_or_default();
        }

        // Restore original includes list for potential future use
        self.includes = Some(processed_includes);
        Ok(())
    }

    /// Merges configuration from an included file with context updates
    ///
    /// 1. Resolves relative paths using module location
    /// 2. Parses included TOML configuration
    /// 3. Merges components with existing configuration
    /// 4. Updates template context with included variables
    ///
    /// # Arguments
    /// * `path` - Path to include file (relative or absolute)
    /// * `context` - Mutable template context updated with included variables
    ///
    /// # Errors
    /// Returns error for:
    /// - Missing include files
    /// - Invalid TOML syntax
    /// - Path resolution failures
    fn merge_include<P>(&mut self, path: P, context: &mut HashMap<String, Value>) -> Result<()>
    where
        P: AsRef<Path>,
    {
        let mut env = HashMap::new();
        env.insert("DOD_CURRENT_MODULE".to_string(), &self.location);

        let expanded_path = file_fs::expand_path(&path, Some(&env))?;
        let abs_path = if expanded_path.is_relative() {
            self.location.join(expanded_path)
        } else {
            expanded_path
        };

        let content = std::fs::read_to_string(&abs_path)
            .wrap_err_with(|| format!("Failed to read include: {}", abs_path.display()))?;

        let mut included: DotdeployModuleBuilder = toml::from_str(&content)
            .wrap_err_with(|| format!("Failed to parse include: {}", abs_path.display()))?;

        // Process nested includes recursively
        if let Some(includes) = included.includes.take() {
            self.includes.get_or_insert_with(Vec::new).extend(includes);
        }

        // Merge other components
        if let Some(files) = included.files.take() {
            let targets: HashSet<_> = files.iter().map(|f| &f.target).collect();
            self.files
                .get_or_insert_with(Vec::new)
                .retain(|f| !targets.contains(&&f.target));
            self.files.get_or_insert_with(Vec::new).extend(files);
        }

        if let Some(tasks) = included.tasks.take() {
            self.tasks.get_or_insert_with(Vec::new).extend(tasks);
        }

        if let Some(messages) = included.messages.take() {
            self.messages.get_or_insert_with(Vec::new).extend(messages);
        }

        if let Some(generators) = included.generators.take() {
            self.generators
                .get_or_insert_with(Vec::new)
                .extend(generators);
        }

        if let Some(packages) = included.packages.take() {
            self.packages.get_or_insert_with(Vec::new).extend(packages);
        }

        if let Some(vars) = included.context_vars.take() {
            context.extend(vars);
        }

        Ok(())
    }

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
                for task_def in task
                    .setup
                    .iter()
                    .chain(task.config.iter())
                    .chain(task.remove.iter())
                    .chain(task.update.iter())
                {
                    if task_def.shell.is_some() & task_def.exec.is_some() {
                        let command_display = match (&task_def.shell, &task_def.exec) {
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
        if let Some(ref mut tasks) = self.tasks {
            for task in tasks {
                task.setup.retain(|t| {
                    t.eval_condition(context, hb).unwrap_or_else(|err| {
                        t.log_error(&self.name, &self.location, err);
                        false
                    })
                });
                task.config.retain(|t| {
                    t.eval_condition(context, hb).unwrap_or_else(|err| {
                        t.log_error(&self.name, &self.location, err);
                        false
                    })
                });
                task.remove.retain(|t| {
                    t.eval_condition(context, hb).unwrap_or_else(|err| {
                        t.log_error(&self.name, &self.location, err);
                        false
                    })
                });
                task.update.retain(|t| {
                    t.eval_condition(context, hb).unwrap_or_else(|err| {
                        t.log_error(&self.name, &self.location, err);
                        false
                    })
                });
            }
        }
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
    includes: Option<Vec<Include>>,
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
        let module = DotdeployModule {
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
