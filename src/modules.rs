use crate::config::DotdeployConfig;
use crate::modules::files::ModuleFile;
use crate::modules::generate_file::Generate;
use crate::modules::messages::ModuleMessage;
use crate::modules::tasks::ModuleTask;
use color_eyre::eyre::{OptionExt, WrapErr, eyre};
use color_eyre::{Result, Section};
use handlebars::Handlebars;
use packages::ModulePackages;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use toml::Value;
use tracing::{error, instrument};

pub(crate) mod files;
mod generate_file;
mod messages;
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
    /// Files to be managed by this module
    pub(crate) files: Option<Vec<ModuleFile>>,
    /// Tasks to be executed by this module
    pub(crate) tasks: Option<Vec<ModuleTask>>,
    /// Messages to be displayed during module execution
    pub(crate) messages: Option<Vec<ModuleMessage>>,
    /// Messages to be displayed during module execution
    pub(crate) generators: Option<Vec<Generate>>,
    // FIXME 2025-03-21: Does this still need to be an Option?
    /// Messages to be displayed during module execution
    pub(crate) packages: Option<Vec<ModulePackages>>,
    /// Key-value pairs used for handlebars templating
    pub(crate) context_vars: Option<HashMap<String, Value>>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum DeployPhase {
    Setup,
    #[default]
    Config,
    Update,
    Remove,
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
                        &file.target
                    ));
                }

                // Check if template is used only for files with type "copy" or "create"
                if file.template.is_some_and(|x| x)
                    & file.operation.as_ref().is_some_and(|x| x == "link")
                {
                    return Err(eyre!(
                        "{}:{}\n Templating is only allowed for operations of type 'create' or 'copy'",
                        &self.location.display(),
                        &file.target
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
                        _ => "<unknown>", // This shouldn't happen due to the condition above
                    };

                    return Err(eyre!(
                        "{}:command={}\n A task can be either a shell command OR an executable",
                        &self.location.display(),
                        command_display
                    ));
                }
            }
        }
        Ok(())
    }

    #[instrument(skip(context))]
    pub(crate) fn eval_conditions<T>(&mut self, context: &T, hb: &Handlebars<'static>) -> Result<()>
    where
        T: Serialize,
    {
        // Evaluate file conditions
        if let Some(ref mut files) = self.files {
            files.retain(|f| {
                f.eval_condition(context, hb).unwrap_or_else(|err| {
                    // Log the error
                    error!(
                        module = self.name,
                        location = self.location.as_path().to_str(),
                        target_file = f.target,
                        "Error during condition evaluation:\n{:?}\n",
                        err
                    );
                    false
                })
            });
        }

        // Evaluate task conditions
        if let Some(ref mut tasks) = self.tasks {
            tasks.retain(|t| {
                t.eval_condition(context, hb).unwrap_or_else(|err| {
                    // Log the error
                    error!(
                        module = self.name,
                        location = self.location.as_path().to_str(),
                        task_exec = match (&t.shell, &t.exec) {
                            (Some(shell), _) => shell,
                            (_, Some(exec)) => exec,
                            _ => "<unknown>",
                        },
                        "Error during condition evaluation:\n{:?}\n",
                        err
                    );
                    false
                })
            });
        }

        // Evaluate message conditions
        if let Some(ref mut messages) = self.messages {
            messages.retain(|m| {
                m.eval_condition(context, hb).unwrap_or_else(|err| {
                    // Log the error
                    error!(
                        module = self.name,
                        location = self.location.as_path().to_str(),
                        msg = m.message,
                        "Error during condition evaluation:\n{:?}\n",
                        err
                    );
                    false
                })
            });
        }

        // Evaluate message conditions
        if let Some(ref mut generators) = self.generators {
            generators.retain(|g| {
                g.eval_condition(context, hb).unwrap_or_else(|err| {
                    // Log the error
                    error!(
                        module = self.name,
                        location = self.location.as_path().to_str(),
                        generate = g.target,
                        "Error during condition evaluation:\n{:?}\n",
                        err
                    );
                    false
                })
            });
        }

        // Evaluate package conditions
        if let Some(ref mut packages) = self.packages {
            packages.retain(|p| {
                p.eval_condition(context, hb).unwrap_or_else(|err| {
                    // Log the error
                    error!(
                        module = self.name,
                        packages = p.install.join(" "),
                        "Error during condition evaluation:\n{:?}\n",
                        err
                    );
                    false
                })
            });
        }

        Ok(())
    }
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct DotdeployModuleBuilder {
    __name__: Option<String>,
    __location__: Option<PathBuf>,
    __reason__: Option<PathBuf>,
    depends_on: Option<Vec<String>>,
    files: Option<Vec<ModuleFile>>,
    tasks: Option<Vec<ModuleTask>>,
    messages: Option<Vec<ModuleMessage>>,
    generators: Option<Vec<Generate>>,
    // FIXME 2025-03-21: Does this still need to be an Option?
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

    pub(crate) fn build(&mut self, reason: String) -> Result<DotdeployModule> {
        Ok(DotdeployModule {
            name: Clone::clone(self.__name__.as_ref().ok_or_eyre("Empty 'name' field")?),
            location: Clone::clone(
                self.__location__
                    .as_ref()
                    .ok_or_eyre("Empty 'location' field")?,
            ),
            reason,
            depends_on: self.depends_on.take(),
            files: self.files.take(),
            tasks: self.tasks.take(),
            messages: self.messages.take(),
            generators: self.generators.take(),
            packages: self.packages.take(),
            context_vars: self.context_vars.take(),
        })
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

/// Provides default deployment phase value ("deploy").
fn default_deploy_phase() -> Option<String> {
    Some("config".to_string())
}

/// Provides default phase step value ("post").
fn default_phase_hook() -> Option<String> {
    Some("post".to_string())
}

/// Provides default file operation value ("link").
fn default_file_operation() -> Option<String> {
    Some("link".to_string())
}

/// Provides default message display_when value ("deploy").
fn default_on_command() -> Option<String> {
    Some("deploy".to_string())
}
