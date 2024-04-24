use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::de::{self, Error, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

// Structs

/// Representation of the configuration for a module.
///
/// This configuration includes optional dependencies, files, hooks, and packages.
#[derive(Deserialize, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ModuleConfig {
    /// A list of module dependencies. Each dependency is identified by its name.
    /// This field is optional.
    pub(crate) depends: Option<Vec<String>>,
    /// A mapping from file destinations to their configurations. This enables detailed
    /// specification of how individual files should be handled during module deployment.
    /// This field is optional.
    // #[serde(default)] is necessary here so the missing field is parsed as None.
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_files")]
    pub(crate) files: Option<BTreeMap<PathBuf, FileConfig>>,
    /// Defines actions to be executed at different phases of the deployment process.
    /// Actions are categorized by their execution stage and further configured by `ActionConfig`.
    /// This field is optional.
    pub(crate) actions: Option<BTreeMap<String, BTreeMap<String, Vec<ActionConfig>>>>,
    /// Specifies packages to be installed as part of the module setup. Packages are
    /// grouped by a key and include conditions for their installation.
    /// This field is optional.
    pub(crate) packages: Option<Vec<Packages>>,
    /// Key values pairs used for handlebars templating
    pub(crate) context_vars: Option<BTreeMap<String, String>>,
    /// Messages to display after module installation or removal
    pub(crate) messages: Option<Vec<Messages>>,
    /// Generate a target file from snippets found in modules
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_files")]
    pub(crate) generate: Option<BTreeMap<PathBuf, Generate>>,
}

/// Configuration for package installation within a module.
///
/// Specifies packages to install and conditional logic for installation.
#[derive(Deserialize, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Packages {
    /// A list of package names to install.
    pub(crate) install: Vec<String>,
    /// A conditional expression evaluated at runtime to determine if the packages should be
    /// installed. The installation proceeds only if this condition evaluates to true.
    /// This field is optional.
    pub(crate) eval_when: Option<String>,
}

/// Configuration for messages within a module.
///
/// Specifies messages and when to display them.
#[derive(Deserialize, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Generate {
    /// Content to prepend.
    pub(crate) prepend: Option<String>,
    /// Source file name.
    pub(crate) source: String,
    /// Content to prepend.
    pub(crate) append: Option<String>,
    /// A conditional expression evaluated at runtime to determine if the file should be generate.
    /// The installation proceeds only if this condition evaluates to true.
    /// This field is optional.
    pub(crate) eval_when: Option<String>,
}

/// Configuration for messages within a module.
///
/// Specifies messages and when to display them.
#[derive(Deserialize, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Messages {
    /// A list of package names to install.
    pub(crate) message: String,
    /// When to display the message. Either "deploy" or "remove". Defaults to "deploy".
    #[serde(default = "default_message")]
    pub(crate) display_when: String,
    /// A conditional expression evaluated at runtime to determine if the message should be
    /// displayed. The installation proceeds only if this condition evaluates to true.
    /// This field is optional.
    pub(crate) eval_when: Option<String>,
}

/// Provides default value for messages.
fn default_message() -> String {
    "deploy".to_string()
}

/// Describes the configuration for a file within a module.
///
/// This includes source location, content, deployment phase, and action type, along with
/// conditional deployment logic.
#[derive(Deserialize, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct FileConfig {
    /// The source path of the file. This field is optional.
    // #[serde(default)] is necessary here so the missing field is parsed as None.
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_source")]
    pub(crate) source: Option<PathBuf>,
    /// The content of the file as a string. This allows direct specification of file content within
    /// the configuration. This field is optional.
    pub(crate) content: Option<String>,
    /// Specifies the deployment phase for the file. Defaults to "deploy".
    #[serde(default = "default_phase")]
    pub(crate) phase: Option<String>,
    /// The action to be taken with this file ("link", "copy" or "create"). Defaults to "link".
    #[serde(default = "default_action")]
    pub(crate) action: Option<String>,
    /// A conditional expression evaluated to decide if the file should be deployed.
    /// Deployment occurs only if this condition evaluates to true.
    /// This field is optional.
    pub(crate) eval_when: Option<String>,
    /// File permissions and ownership.
    pub(crate) permissions: Option<FilePermissions>,
    /// If file is a template
    #[serde(default = "default_template")]
    pub(crate) template: Option<bool>,
}

/// Provides default value for template.
fn default_template() -> Option<bool> {
    Some(false)
}

/// File permission representation
///
/// This includes owner and group as well as access permissions
#[derive(Deserialize, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct FilePermissions {
    pub(crate) owner: Option<String>,
    pub(crate) group: Option<String>,
    pub(crate) permissions: Option<String>,
}

// Default values for FileConfig
/// Provides default values for the deployment phase of a file.
fn default_phase() -> Option<String> {
    Some("deploy".to_string())
}

/// Provides default values for the deployment action of a file.
fn default_action() -> Option<String> {
    Some("link".to_string())
}

/// Represents an individual action within a deployment process.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub(crate) struct ActionConfig {
    /// The command(s) to be executed by the action.
    pub(crate) exec: RunExec,
    /// If the command should be run with sudo
    sudo: bool,
    /// Additional arguments passed
    args: Option<Vec<String>>,
    /// A conditional expression that determines if the action should be executed.
    /// This field is optional.
    eval_when: Option<String>,
}

#[derive(Deserialize, Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub(crate) enum RunExec {
    Code(String),
    File(String),
}

// Implementing Deserialize trait for ActionConfig to provide custom deserialization logic.
impl<'de> Deserialize<'de> for ActionConfig {
    // This is where the deserialization process begins.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // A temporary struct to help deserialize the raw data. It mirrors the structure of
        // ActionConfig but includes an additional `exec_file` field to help determine how to
        // interpret the `exec` field.
        #[derive(Deserialize)]
        struct Helper {
            exec: String,              // Raw command or filepath as a string
            exec_file: Option<bool>,   // Indicator if exec is a file
            sudo: Option<bool>,        // Indicator if sudo should be used
            args: Option<Vec<String>>, // Indicator if additional args should be used
            eval_when: Option<String>, // Optional condition for execution
        }

        // Visitor struct for custom processing of the deserialized data.
        struct ActionConfigVisitor;

        // Implementation of Visitor trait for ActionConfigVisitor.
        impl<'de> Visitor<'de> for ActionConfigVisitor {
            type Value = ActionConfig;

            // Describes what this visitor expects to receive.
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct ActionConfig")
            }

            // Custom processing of the map representing deserialized data.
            fn visit_map<V>(self, mut map: V) -> Result<ActionConfig, V::Error>
            where
                V: MapAccess<'de>,
            {
                // Deserialize the map into our helper struct.
                let helper: Helper =
                    Deserialize::deserialize(de::value::MapAccessDeserializer::new(&mut map))?;

                // Decide how to interpret the `exec` field based on `exec_file`. If `exec_file` is
                // true, treat `exec` as a filepath (File variant of Exec). Otherwise, treat `exec`
                // as a command to execute directly (Code variant of Exec).
                let exec = if helper.exec_file.is_some_and(|x| x == true) {
                    match shellexpand::full(&helper.exec) {
                        Ok(p) => RunExec::File(p.to_string()),
                        Err(e) => {
                            return Err(V::Error::custom(format!("Error expanding path: {}", e)))
                        }
                    }
                } else {
                    RunExec::Code(helper.exec)
                };

                // Construct and return the actual ActionConfig object with our custom logic
                // applied.
                Ok(ActionConfig {
                    exec,
                    eval_when: helper.eval_when,
                    sudo: helper.sudo.unwrap_or_else(|| false),
                    args: helper.args,
                })
            }
        }

        // Trigger the custom deserialization process using the visitor pattern. This line
        // effectively starts the deserialization process defined above.
        deserializer.deserialize_map(ActionConfigVisitor)
    }
}

/// Custom deserializer for the `source` field in `FileConfig`
fn deserialize_source<'de, D>(deserializer: D) -> Result<Option<PathBuf>, D::Error>
where
    D: Deserializer<'de>,
{
    let source: Option<String> = Option::deserialize(deserializer)?;
    source
        .map(|s| {
            if shellexpand::full(&s)?.starts_with("/") {
                shellexpand::full(&s).map(|expanded| PathBuf::from(expanded.as_ref()))
            } else {
                shellexpand::full(&format!(
                    "{}{}{}",
                    std::env::var("DOD_CURRENT_MODULE").expect(
                        "env variable `DOD_CURRENT_MODULE` should be set by `modules::add_module`"
                    ),
                    std::path::MAIN_SEPARATOR_STR,
                    &s
                ))
                .map(|expanded| PathBuf::from(expanded.as_ref()))
            }
        })
        .transpose()
        .map_err(serde::de::Error::custom)
}

/// Custom deserializer for the BTreeMap keys (file destinations)
fn deserialize_files<'de, D, T>(deserializer: D) -> Result<Option<BTreeMap<PathBuf, T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    // Deserialize into map
    let opt_map: Option<BTreeMap<String, T>> = Option::deserialize(deserializer)?;
    // Run shellexpand on keys (file destinations)
    opt_map
        .map(|map| {
            map.into_iter()
                .map(|(key, value)| {
                    shellexpand::full(&key)
                        .map(|expanded| (PathBuf::from(expanded.as_ref()), value))
                        .map_err(serde::de::Error::custom)
                })
                .collect()
        })
        // Transpose Option to Result
        .transpose()
}

// Implementations

/// A trait for items that include a conditional execution field (`eval_when`).
trait Conditional {
    fn eval_when(&self) -> &Option<String>;
}

/// Implementation of `Conditional` for `FileConfig`, providing access to its `eval_when` field.
impl Conditional for FileConfig {
    fn eval_when(&self) -> &Option<String> {
        &self.eval_when
    }
}

/// Implementation of `Conditional` for `ActionConfig`, providing access to its `eval_when` field.
impl Conditional for ActionConfig {
    fn eval_when(&self) -> &Option<String> {
        &self.eval_when
    }
}

/// Implementation of `Conditional` for `Packages`, providing access to its `eval_when` field.
impl Conditional for Packages {
    fn eval_when(&self) -> &Option<String> {
        &self.eval_when
    }
}

/// Implementation of `Conditional` for `Packages`, providing access to its `eval_when` field.
impl Conditional for Messages {
    fn eval_when(&self) -> &Option<String> {
        &self.eval_when
    }
}

/// Implementation of `Conditional` for `Generate`, providing access to its `eval_when` field.
impl Conditional for Generate {
    fn eval_when(&self) -> &Option<String> {
        &self.eval_when
    }
}

/// Evaluate the handlebars template in the `eval_when` field.
fn eval_condition(
    condition: &str,
    context: &serde_json::Value,
    hb: &handlebars::Handlebars<'static>,
) -> Result<bool> {
    let eval_template = format!(
        "{{{{#if {condition}}}}}true{{{{else}}}}false{{{{/if}}}}",
        condition = condition
    );
    let result = hb
        .render_template(&eval_template, &context)
        .with_context(|| format!("Failed to evaluate template: {}", eval_template))?;

    match result.as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Ok(false),
    }
}

impl ModuleConfig {
    /// Evaluate conditionals for files, hooks and packages.
    pub(crate) fn eval_conditionals(
        &mut self,
        context: &serde_json::Value,
        hb: &handlebars::Handlebars<'static>,
    ) -> Result<()> {
        match &mut self.files {
            Some(files) => {
                files.retain(|_key, value| {
                    if let Some(cond) = value.eval_when() {
                        match eval_condition(cond, context, hb) {
                            // Condition passes, so do not remove
                            Ok(true) => true,
                            // Condition fails, remove
                            Ok(false) => false,
                            // Display error, but remove and continue
                            Err(e) => {
                                error!("Error during condtition evaluation:\n {}", e);
                                false
                            }
                        }
                    } else {
                        // No condition means we do not remove it
                        true
                    }
                });
                // Rempve potentially empty btreemap
                if files.is_empty() {
                    self.files = None
                }
            }
            None => (),
        }
        match &mut self.generate {
            Some(generate) => {
                generate.retain(|_key, value| {
                    if let Some(cond) = value.eval_when() {
                        match eval_condition(cond, context, hb) {
                            // Condition passes, so do not remove
                            Ok(true) => true,
                            // Condition fails, remove
                            Ok(false) => false,
                            // Display error, but remove and continue
                            Err(e) => {
                                error!("Error during condtition evaluation:\n {}", e);
                                false
                            }
                        }
                    } else {
                        // No condition means we do not remove it
                        true
                    }
                });
                // Rempve potentially empty btreemap
                if generate.is_empty() {
                    self.generate = None
                }
            }
            None => (),
        }
        match &mut self.actions {
            Some(actions) => {
                // Iterate through each phase and its stages
                for (_, stages) in actions.iter_mut() {
                    // Iterate through each stage
                    for (_, stage_actions) in stages.iter_mut() {
                        // Process each action
                        stage_actions.retain(|action| {
                            if let Some(cond) = &action.eval_when {
                                match eval_condition(cond, context, hb) {
                                    // Condition passes, so do not remove
                                    Ok(true) => true,
                                    // Condition fails, remove
                                    Ok(false) => false,
                                    // Display error, but remove and continue
                                    Err(e) => {
                                        error!("Error during condtition evaluation:\n {}", e);
                                        false
                                    }
                                }
                            } else {
                                // No condition means we retain it
                                true
                            }
                        });
                    }
                    // Remove stages if they become empty
                    stages.retain(|_, stage_actions| !stage_actions.is_empty());
                }
                // Remove phases if they become empty
                actions.retain(|_, stages| !stages.is_empty());
                if actions.is_empty() {
                    self.actions = None;
                }
            }
            None => (),
        }
        match &mut self.packages {
            Some(packages) => {
                packages.retain(|pkgs| {
                    if let Some(cond) = &pkgs.eval_when {
                        match eval_condition(cond, context, hb) {
                            // Condition passes, so do not remove
                            Ok(true) => true,
                            // Condition fails, remove
                            Ok(false) => false,
                            // Display error, but remove and continue
                            Err(e) => {
                                error!("Error during condtition evaluation:\n {}", e);
                                false
                            }
                        }
                    } else {
                        // No condition means we retain it
                        true
                    }
                });
                // Rempve potentially empty btreemap
                if packages.is_empty() {
                    self.packages = None
                }
            }
            None => (),
        }
        match &mut self.messages {
            Some(messages) => {
                messages.retain(|msgs| {
                    if let Some(cond) = &msgs.eval_when {
                        match eval_condition(cond, context, hb) {
                            // Condition passes, so do not remove
                            Ok(true) => true,
                            // Condition fails, remove
                            Ok(false) => false,
                            // Display error, but remove and continue
                            Err(e) => {
                                error!("Error during condtition evaluation:\n {}", e);
                                false
                            }
                        }
                    } else {
                        // No condition means we retain it
                        true
                    }
                });
                // Rempve potentially empty btreemap
                if messages.is_empty() {
                    self.messages = None
                }
            }
            None => (),
        }
        Ok(())
    }

    /// Read the module configuration file and return a [ModuleConfig].
    pub(crate) fn read_config<P: AsRef<Path>>(path: P) -> Result<ModuleConfig> {
        let config_path = path.as_ref().join("config.toml");
        let toml_string = std::fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read module config file: {:?}", config_path))?;
        let mut config: ModuleConfig = toml::from_str(&toml_string)
            .with_context(|| format!("Failed to parse module config from: {}", toml_string))?;

        // Expand directories in destination and source fields to separate files
        let mut to_expand = vec![];
        if let Some(files) = &config.files {
            for (dest, conf) in files.iter() {
                if dest
                    .to_str()
                    .ok_or_else(|| anyhow!("Filename contains invalid Unicode characters"))?
                    .ends_with("*")
                {
                    if let Some(s) = &conf.source {
                        if s.to_str()
                            .ok_or_else(|| anyhow!("Filename contains invalid Unicode characters"))?
                            .ends_with("*")
                        {
                            to_expand.push(dest.clone());
                        }
                    } else {
                        bail!("Directory expansion syntax can only be used if both destination and source are using it.")
                    }
                }
            }
        }

        // Process the expansions
        if !to_expand.is_empty() {
            if let Some(files) = &mut config.files {
                for key in to_expand {
                    if let Some((dest, conf)) = files.remove_entry(&key) {
                        let source_parent = conf
                            .source
                            .as_ref()
                            .and_then(|s| s.parent())
                            .ok_or_else(|| {
                                anyhow!("Source {:?} has no parent directory", conf.source.as_ref())
                            })?;
                        let dest_parent = dest.parent().ok_or_else(|| {
                            anyhow!("Destination {:?} has no parent directory", dest)
                        })?;
                        let new_sources =
                            read_directory(conf.source.as_ref().unwrap().parent().unwrap())
                                .with_context(|| {
                                    format!(
                                        "Failed to read directory {:?}",
                                        conf.source.as_ref().unwrap().parent().unwrap()
                                    )
                                })?;
                        for s in new_sources.into_iter() {
                            let relative_path = s.strip_prefix(source_parent)?.to_owned();
                            let new_dest = dest_parent.join(relative_path);
                            files.insert(
                                new_dest,
                                FileConfig {
                                    source: Some(s.to_owned()),
                                    content: conf.content.clone(),
                                    phase: conf.phase.clone(),
                                    action: conf.action.clone(),
                                    eval_when: conf.eval_when.clone(),
                                    permissions: if conf.permissions.is_some() {
                                        Some(FilePermissions {
                                            owner: conf.permissions.as_ref().unwrap().owner.clone(),
                                            group: conf.permissions.as_ref().unwrap().group.clone(),
                                            permissions: conf
                                                .permissions
                                                .as_ref()
                                                .unwrap()
                                                .permissions
                                                .clone(),
                                        })
                                    } else {
                                        None
                                    },
                                    template: conf.template.clone(),
                                },
                            );
                        }
                    }
                }
            }
        }
        Ok(config)
    }
}

/// Read all files in a directory recursively
fn read_directory(path: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            files.extend(read_directory(&path)?);
        } else {
            files.push(path)
        }
    }
    Ok(files)
}

impl ActionConfig {
    pub(crate) async fn run(&self) -> Result<()> {
        match &self.exec {
            RunExec::Code(code) => {
                let mut cmd = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(code)
                    .spawn()
                    .with_context(|| format!("Failed to run {:?}", &self.exec))?;

                // It's streaming here
                let status = cmd.wait()?;
                if status.success() {
                    Ok(())
                } else {
                    bail!("Failed to executed {:?}", code);
                }
            }
            RunExec::File(file) => {
                let args = self.args.as_deref().unwrap_or(&[]);
                if self.sudo {
                    crate::sudo::spawn_sudo_maybe(format!(
                        "Running {:?} with args: {:?}",
                        file, args
                    ))
                    .await
                    .context("Failed to spawn sudo")?;

                    let mut fcmd = vec![file];
                    fcmd.extend(args);

                    let mut cmd = std::process::Command::new("sudo")
                        .args(&fcmd)
                        .spawn()
                        .with_context(|| format!("Failed to run {:?}", fcmd))?;
                    // It's streaming here
                    let status = cmd.wait()?;
                    if status.success() {
                        Ok(())
                    } else {
                        bail!("Failed to executed {:?} with args {:?}", file, args);
                    }
                } else {
                    let mut cmd = std::process::Command::new(&file)
                        .args(args)
                        .spawn()
                        .with_context(|| {
                            format!("Failed to run {:?} with args {:?}", file, args)
                        })?;

                    // It's streaming here
                    let status = cmd.wait()?;
                    if status.success() {
                        Ok(())
                    } else {
                        bail!("Failed to executed {:?} with args {:?}", file, args);
                    }
                }
            }
        }
    }
}

// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Helper function to create a testing context
    fn test_context() -> serde_json::Value {
        json!({
            "condition_met": true,
            "condition_not_met": false,
        })
    }

    #[test]
    fn test_deserialize_config() {
        let toml_string = r#"
            depends = ["foo"]

            [[packages]]
            install = ["pkg1", "pkg2"]
            eval_when = "condition_met"

            [files."file1.txt"]
            source = "source1"
            eval_when = "condition_met"

            [[actions.setup.pre]]
            exec = "command1"
            eval_when = "condition_not_met"
        "#;

        let config: ModuleConfig = toml::from_str(toml_string).unwrap();
        assert_eq!(config.depends.unwrap(), vec!["foo"]);
        assert!(config.packages.is_some());
        assert!(config.files.is_some());
        assert!(config.actions.is_some());
    }

    #[test]
    fn test_default_values() {
        let file_config = FileConfig {
            source: None,
            content: None,
            phase: None,
            action: None,
            eval_when: None,
            permissions: None,
            template: None,
        };

        assert_eq!(
            file_config
                .phase
                .unwrap_or_else(|| default_phase().unwrap()),
            "deploy"
        );
        assert_eq!(
            file_config
                .action
                .unwrap_or_else(|| default_action().unwrap()),
            "link"
        );
    }

    #[test]
    fn test_eval_conditionals() -> Result<()> {
        let mut config = ModuleConfig {
            depends: Some(vec!["foo".to_string()]),
            packages: Some(vec![Packages {
                install: vec![],
                eval_when: Some("condition_not_met".to_string()),
            }]),
            files: Some(BTreeMap::from([(
                PathBuf::from("file1.txt"),
                FileConfig {
                    source: Some(PathBuf::from("source1")),
                    content: None,
                    phase: Some("deploy".to_string()),
                    action: Some("link".to_string()),
                    permissions: None,
                    eval_when: Some("condition_not_met".to_string()),
                    template: None,
                },
            )])),
            actions: Some(BTreeMap::from([(
                "setup".to_string(),
                BTreeMap::from([(
                    "pre".to_string(),
                    vec![ActionConfig {
                        exec: RunExec::Code("command1".to_string()),
                        eval_when: Some("condition_met".to_string()),
                        sudo: false,
                        args: None,
                    }],
                )]),
            )])),
            context_vars: None,
            messages: None,
            generate: None,
        };

        let context = test_context();
        let mut handlebars: handlebars::Handlebars<'static> = handlebars::Handlebars::new();
        handlebars.set_strict_mode(true);
        config.eval_conditionals(&context, &handlebars)?;

        assert!(config.packages.is_none());
        assert!(config.files.is_none());
        assert!(config.actions.is_some());
        Ok(())
    }
}
