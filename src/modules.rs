//! This module defines the structure and operations for managing Dotdeploy modules.

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::read_module;

/// Represents a Dotdeploy module with its properties and configuration.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Module {
    /// The name of the module
    pub(crate) name: String,
    /// The file system path to the module
    pub(crate) location: PathBuf,
    /// The reason for adding this module (e.g., "manual" or "automatic")
    pub(crate) reason: String,
    /// The parsed configuration of the module
    pub(crate) config: read_module::ModuleConfig,
}

/// Adds a module and its dependencies to the provided set of modules.
///
/// This function reads the module's configuration, processes its dependencies recursively, and adds
/// the module to the set. It also updates the context with any variables defined in the module's
/// configuration.
///
/// # Parameters
///
/// - `module_name`: The name of the module to be added.
/// - `dotdeploy_config`: Reference to the global Dotdeploy configuration.
/// - `modules`: A mutable reference to a BTreeSet where modules will be inserted.
/// - `manual`: A boolean indicating if the module is being added manually.
/// - `context`: A mutable reference to a map for storing context variables.
///
/// # Returns
///
/// A Result indicating success or failure of the module addition process.
///
/// # Errors
///
/// Returns an error if the module's configuration cannot be read or processed.
pub(crate) fn add_module(
    module_name: &str,
    dotdeploy_config: &crate::config::ConfigFile,
    modules: &mut std::collections::BTreeSet<Module>,
    manual: bool,
    context: &mut std::collections::BTreeMap<String, String>,
) -> Result<()> {
    debug!("Processing module: {}", module_name);

    // Determine the path to the module's configuration file
    let path: PathBuf = if module_name.starts_with("hosts") {
        // For host-specific modules
        dotdeploy_config
            .hosts_root
            .join(module_name.trim_start_matches("hosts/"))
    } else {
        // For regular modules
        dotdeploy_config.modules_root.join(module_name)
    };

    // Set the current module path as an environment variable
    std::env::set_var("DOD_CURRENT_MODULE", &path);

    // Attempt to read and process the module's configuration
    match read_module::ModuleConfig::read_config(&path) {
        Ok(config) => {
            // Extract and add context variables from the module config
            if let Some(ref vars) = config.context_vars {
                for (k, v) in vars.iter() {
                    context.insert(k.to_string(), v.to_string());
                }
            }

            // Recursively process dependencies
            if let Some(dependencies) = &config.depends {
                for dependency in dependencies {
                    add_module(dependency, dotdeploy_config, modules, false, context)?;
                }
            }

            // Add the current module to the set
            modules.insert(Module {
                name: module_name.to_string(),
                location: path,
                config,
                reason: if manual { "manual" } else { "automatic" }.to_string(),
            });
            Ok(())
        }
        Err(e) => Err(e)
            .with_context(|| format!("Failed to read module configuration for {}", module_name)),
    }
}

//
// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;
    use std::fs;
    use tempfile::tempdir;

    #[derive(Serialize)]
    struct TempModuleConfig {
        depends: Option<Vec<String>>,
    }

    fn create_temp_module_config(
        dir: &tempfile::TempDir,
        name: &str,
        depends: Option<Vec<&str>>,
    ) -> PathBuf {
        let config = TempModuleConfig {
            depends: depends.map(|deps| deps.into_iter().map(String::from).collect()),
        };

        let mut path = dir.path().join(name);
        fs::create_dir_all(&path).expect("Failed to create temporary module directory");
        path = path.join("config").with_extension("toml");
        let config_str = toml::to_string(&config).expect("Failed to serialize temp module config");
        fs::write(&path, config_str).expect("Failed to write temp module config");
        path
    }

    #[test]
    fn test_add_module_with_dependencies() -> Result<()> {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let dotdeploy_config = crate::config::ConfigFile {
            config_root: temp_dir.path().to_path_buf(),
            hosts_root: temp_dir.path().to_path_buf(),
            modules_root: temp_dir.path().to_path_buf(),
            distribution: "None".to_string(),
            hostname: "None".to_string(),
            use_sudo: true,
            deploy_sys_files: true,
            skip_pkg_install: false,
            intall_pkg_cmd: None,
            remove_pkg_cmd: None,
        };

        let mut modules = std::collections::BTreeSet::new();
        let mut context = std::collections::BTreeMap::new();

        // Create temporary module files with dependencies
        create_temp_module_config(&temp_dir, "module1", Some(vec!["module2"]));
        create_temp_module_config(&temp_dir, "module2", Some(vec!["module3"]));
        create_temp_module_config(&temp_dir, "module3", None);

        // Add the root module, which should recursively add its dependencies
        add_module(
            "module1",
            &dotdeploy_config,
            &mut modules,
            true,
            &mut context,
        )?;

        // Check that all modules are present in the set
        assert!(modules.iter().any(|m| m.name == "module1"));
        assert!(modules.iter().any(|m| m.name == "module2"));
        assert!(modules.iter().any(|m| m.name == "module3"));

        // Test that adding modules does not cause duplicates
        add_module(
            "module1",
            &dotdeploy_config,
            &mut modules,
            true,
            &mut context,
        )?;
        // Verify length of the set
        assert_eq!(modules.len(), 3);
        Ok(())
    }
}
