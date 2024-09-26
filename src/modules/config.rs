//! This module defines the configuration structure and related functions for dotdeploy modules.
//!
//! It handles parsing, evaluation, and manipulation of module configurations, including
//! deserialization of file paths, conditional evaluation, and directory wildcard expansion.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Deserializer};

use crate::modules::actions::ModuleAction;
use crate::modules::conditional::{ConditionalEvaluator, DefaultConditionalEvaluator};
use crate::modules::files::{FilePermissions, ModuleFile};
use crate::modules::generate::Generate;
use crate::modules::messages::ModuleMessages;
use crate::modules::packages::ModulePackages;
use crate::utils::file_fs;

/// Representation of the configuration for a module.
///
/// This configuration includes optional dependencies, files, hooks, and packages.
#[derive(Deserialize, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ModuleConfig {
    /// A list of module dependencies. Each dependency is identified by its name.
    pub(crate) depends: Option<Vec<String>>,
    /// A mapping from file destinations to their configurations.
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_files")]
    pub(crate) files: Option<BTreeMap<PathBuf, ModuleFile>>,
    /// Defines actions to be executed at different phases of the deployment process.
    pub(crate) actions: Option<BTreeMap<String, BTreeMap<String, Vec<ModuleAction>>>>,
    /// Specifies packages to be installed as part of the module setup.
    pub(crate) packages: Option<Vec<ModulePackages>>,
    /// Key-value pairs used for handlebars templating.
    pub(crate) context_vars: Option<BTreeMap<String, String>>,
    /// Messages to display after module installation or removal.
    pub(crate) messages: Option<Vec<ModuleMessages>>,
    /// Generate a target file from snippets found in modules.
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_files")]
    pub(crate) generate: Option<BTreeMap<PathBuf, Generate>>,
}

/// Custom deserializer for file paths in the configuration.
///
/// This function deserializes file paths, expanding shell variables in the process.
fn deserialize_files<'de, D, T>(deserializer: D) -> Result<Option<BTreeMap<PathBuf, T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    // First, deserialize the input into an Option<BTreeMap<String, T>>
    let opt_map: Option<BTreeMap<String, T>> = Option::deserialize(deserializer)?;

    // If the option is None, return None early
    if opt_map.is_none() {
        return Ok(None);
    }

    // Unwrap the option to get the BTreeMap
    let map = opt_map.unwrap();

    // Process each entry in the map
    let processed_map: Result<BTreeMap<PathBuf, T>, D::Error> = map
        .into_iter()
        .map(|(key, value)| {
            // For each key (file path), expand any shell variables
            shellexpand::full(&key)
                .map(|expanded| {
                    // Convert the expanded string to a PathBuf
                    (PathBuf::from(expanded.as_ref()), value)
                })
                .map_err(serde::de::Error::custom)
        })
        .collect(); // Collect results, propagating any errors

    // Wrap the processed map in Some and return
    Ok(Some(processed_map?))
}

impl ModuleConfig {
    /// Evaluates conditional logic in the configuration.
    ///
    /// This method processes each section of the configuration that can contain conditional logic,
    /// removing items that don't meet their conditions.
    pub(crate) fn eval_conditionals(
        &mut self,
        context: &serde_json::Value,
        hb: &handlebars::Handlebars<'static>,
    ) -> Result<()> {
        let evaluator = DefaultConditionalEvaluator;

        // Evaluate conditionals for each section of the configuration
        self.files = evaluator.eval_conditional_map(self.files.take(), context, hb)?;
        self.generate = evaluator.eval_conditional_map(self.generate.take(), context, hb)?;
        self.actions = evaluator.eval_conditional_nested_map(self.actions.take(), context, hb)?;
        self.packages = evaluator.eval_conditional_vec(self.packages.take(), context, hb)?;
        self.messages = evaluator.eval_conditional_vec(self.messages.take(), context, hb)?;

        Ok(())
    }

    /// Reads and parses the module configuration from a TOML file.
    ///
    /// This method reads the 'config.toml' file from the specified path, deserializes it into a
    /// ModuleConfig struct, and expands any directory wildcards.
    pub(crate) fn read_config<P: AsRef<Path>>(path: P) -> Result<Self> {
        let config_path = path.as_ref().join("config.toml");

        // Read the contents of the config file
        let toml_string = std::fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read module config file: {:?}", config_path))?;

        // Parse the TOML string into a ModuleConfig struct
        let mut config: ModuleConfig = toml::from_str(&toml_string)
            .with_context(|| format!("Failed to parse module config from: {}", toml_string))?;

        // Expand any directory wildcards in the configuration
        config.expand_directory_wildcards()?;

        Ok(config)
    }

    /// Expands directory wildcards in the file configurations.
    ///
    /// This method processes any file paths ending with '*' and expands them to include all files
    /// in the specified directory.
    fn expand_directory_wildcards(&mut self) -> Result<()> {
        if let Some(files) = &mut self.files {
            // Collect all expansions that need to be made
            let expansions: Result<Vec<(PathBuf, Vec<(PathBuf, ModuleFile)>)>> = files
                .iter()
                .filter(|(dest, conf)| {
                    // Filter for paths ending with '*'
                    dest.to_str().map(|s| s.ends_with('*')).unwrap_or(false)
                        && conf
                            .source
                            .as_ref()
                            .map(|s| s.to_str().map(|t| t.ends_with('*')).unwrap_or(false))
                            .unwrap_or(false)
                })
                .map(|(dest, conf)| {
                    // Expand each wildcard directory
                    Self::expand_single_directory(dest, conf)
                        .map(|expanded| (dest.clone(), expanded))
                })
                .collect();

            let expansions = expansions?;

            // Apply the expansions to the files map
            for (key, expanded_files) in expansions {
                files.remove(&key);
                files.extend(expanded_files);
            }
        }
        Ok(())
    }

    /// Expands a single directory wildcard.
    ///
    /// This method takes a destination path and file configuration with wildcards and expands them
    /// to include all files in the specified directory.
    fn expand_single_directory(
        dest: &PathBuf,
        conf: &ModuleFile,
    ) -> Result<Vec<(PathBuf, ModuleFile)>> {
        // Get the parent directories of the source and destination
        let source_parent = conf
            .source
            .as_ref()
            .and_then(|s| s.parent())
            .ok_or_else(|| anyhow!("Source {:?} has no parent directory", conf.source.as_ref()))?;
        let dest_parent = dest
            .parent()
            .ok_or_else(|| anyhow!("Destination {:?} has no parent directory", dest))?;

        // Read all files in the source directory
        let new_sources = file_fs::read_directory(conf.source.as_ref().unwrap().parent().unwrap())
            .with_context(|| {
                format!(
                    "Failed to read directory {:?}",
                    conf.source.as_ref().unwrap().parent().unwrap()
                )
            })?;

        // Create new ModuleFile for each file found
        let mut expanded_files = Vec::new();
        for s in new_sources {
            let relative_path = s.strip_prefix(source_parent)?.to_owned();
            let new_dest = dest_parent.join(relative_path);
            expanded_files.push((
                new_dest,
                ModuleFile {
                    source: Some(s.to_owned()),
                    content: conf.content.clone(),
                    phase: conf.phase.clone(),
                    action: conf.action.clone(),
                    eval_when: conf.eval_when.clone(),
                    permissions: conf.permissions.as_ref().map(|p| FilePermissions {
                        owner: p.owner.clone(),
                        group: p.group.clone(),
                        permissions: p.permissions.clone(),
                    }),
                    template: conf.template,
                },
            ));
        }
        Ok(expanded_files)
    }
}

//
// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_deserialize_files() {
        let json = r#"
        {
            "~/file1.txt": {"content": "Hello"},
            "$HOME/file2.txt": {"content": "World"}
        }
        "#;

        let mut deserializer = serde_json::Deserializer::from_str(json);
        let result: Option<BTreeMap<PathBuf, serde_json::Value>> =
            deserialize_files(&mut deserializer).unwrap();

        assert!(result.is_some());
        let map = result.unwrap();
        assert_eq!(map.len(), 2);
        assert!(map.contains_key(&PathBuf::from(
            shellexpand::tilde("~/file1.txt").to_string()
        )));
        assert!(map.contains_key(&PathBuf::from(
            shellexpand::env("$HOME/file2.txt").unwrap().to_string()
        )));
    }

    #[test]
    fn test_eval_conditionals() {
        let mut config = ModuleConfig {
            files: Some(BTreeMap::from([
                (
                    PathBuf::from("/test1"),
                    ModuleFile {
                        eval_when: Some("condition_met".to_string()),
                        ..Default::default()
                    },
                ),
                (
                    PathBuf::from("/test2"),
                    ModuleFile {
                        eval_when: Some("condition_not_met".to_string()),
                        ..Default::default()
                    },
                ),
            ])),
            ..Default::default()
        };

        let context = serde_json::json!({
            "condition_met": true,
            "condition_not_met": false,
        });
        let handlebars = handlebars::Handlebars::new();

        config.eval_conditionals(&context, &handlebars).unwrap();

        assert!(config.files.is_some());
        assert!(config
            .files
            .as_ref()
            .unwrap()
            .contains_key(&PathBuf::from("/test1")));
        assert!(!config
            .files
            .as_ref()
            .unwrap()
            .contains_key(&PathBuf::from("/test2")));
    }

    #[test]
    fn test_read_config() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let config_path = temp_dir.path().join("config.toml");
        std::fs::create_dir_all(temp_dir.path().join("test/"))?;
        std::fs::File::create(temp_dir.path().join("test").join("file1.txt"))?;
        std::fs::create_dir_all(temp_dir.path().join("source/"))?;
        std::fs::File::create(temp_dir.path().join("source").join("file1.txt"))?;

        fs::write(
            &config_path,
            format!(
                r#"
            [files."{}/test/*"]
            source = "{}/source/*"
            "#,
                temp_dir.path().to_string_lossy(),
                temp_dir.path().to_string_lossy()
            ),
        )?;

        let config = ModuleConfig::read_config(temp_dir.path())?;

        assert!(config.files.is_some());
        assert_eq!(config.files.as_ref().unwrap().len(), 1);
        assert!(config
            .files
            .as_ref()
            .unwrap()
            .contains_key(&PathBuf::from(format!(
                "{}/test/file1.txt",
                temp_dir.path().to_string_lossy()
            ))));

        Ok(())
    }

    #[test]
    fn test_expand_directory_wildcards() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let source_dir = temp_dir.path().join("source");
        let dest_dir = temp_dir.path().join("dest");

        fs::create_dir(&source_dir)?;
        fs::write(source_dir.join("file1.txt"), "content1")?;
        fs::write(source_dir.join("file2.txt"), "content2")?;

        let mut config = ModuleConfig {
            files: Some(BTreeMap::from([(
                dest_dir.join("*"),
                ModuleFile {
                    source: Some(source_dir.join("*")),
                    ..Default::default()
                },
            )])),
            ..Default::default()
        };

        config.expand_directory_wildcards()?;

        assert_eq!(config.files.as_ref().unwrap().len(), 2);
        assert!(config
            .files
            .as_ref()
            .unwrap()
            .contains_key(&dest_dir.join("file1.txt")));
        assert!(config
            .files
            .as_ref()
            .unwrap()
            .contains_key(&dest_dir.join("file2.txt")));

        Ok(())
    }
}
