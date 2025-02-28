//! This module defines the structure and operations for managing the modules queue.
//!
//! The queue holds all modules and their configurations to be deployed. It provides methods to add
//! modules to the queue and process them, handling dependencies and context variables.

use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use crate::config::DotdeployConfig;
use crate::modules::Module;
use crate::modules::config::ModuleConfig;

/// Represents a queue of modules to be processed for deployment.
#[derive(Debug)]
pub(crate) struct ModuleQueue {
    /// A set of modules, ordered by their natural ordering.
    pub(crate) modules: BTreeSet<Module>,
    /// A map of context variables shared across modules.
    pub(crate) context: BTreeMap<String, String>,
}

impl ModuleQueue {
    /// Adds modules to the queue, processing their dependencies recursively.
    ///
    /// # Arguments
    ///
    /// * `module_names` - A vector of module names to be added.
    /// * `dotdeploy_config` - The global configuration for dotdeploy.
    /// * `manual` - A flag indicating whether the modules are being added manually or
    ///   automatically.
    ///
    /// # Returns
    ///
    /// A Result indicating success or containing an error if module processing fails.
    pub(crate) fn add_modules(
        &mut self,
        module_names: &Vec<String>,
        dotdeploy_config: &DotdeployConfig,
        manual: bool,
    ) -> Result<()> {
        // Iterate over each module name provided
        for module_name in module_names {
            // Determine the filesystem location of the module
            let path = self
                .locate_module(&module_name, dotdeploy_config)
                .with_context(|| format!("Failed to locate module {}", module_name))?;

            // Set an environment variable with the current module's path
            // This is used by other parts of the application that need to know the current context
            unsafe {
                std::env::set_var("DOD_CURRENT_MODULE", &path);
            }

            // Read and parse the module's configuration file
            let config = ModuleConfig::read_config(&path).with_context(|| {
                format!(
                    "Failed to read module configuration for {} from {:?}",
                    module_name, &path
                )
            })?;

            // Extract the module dependencies
            let dependencies = config.depends.clone();

            // Extract context variables from the module's configuration
            // These variables can be used for templating or conditional logic in other parts of the
            // deployment process
            if let Some(ref vars) = config.context_vars {
                for (k, v) in vars.iter() {
                    self.context.insert(k.to_string(), v.to_string());
                }
            }

            // Create a new Module instance and add it to the queue
            let module = Module {
                name: module_name.to_string(),
                location: path,
                config,
                // Set the reason for adding this module (manual or automatic)
                reason: if manual { "manual" } else { "automatic" }.to_string(),
            };

            // Check if the module already exists
            if let Some(existing_module) = self.modules.get(&module) {
                // If the existing module was added automatically and this one is manual,
                // remove the existing one and add the new one
                if existing_module.reason == "automatic" && manual {
                    self.modules.remove(&module);
                    self.modules.insert(module);
                    // Automatically modules should only be added during recursion so we can asume
                    // that we visited this module already and thus, can skip it.
                    continue;
                } else {
                    // If the module exists already, we can skip it.
                    continue;
                }
            } else {
                // If the module doesn't exist, add it
                self.modules.insert(module);
            }

            // If the module has dependencies, process them recursively and add them to the queue.
            if let Some(dependencies) = &dependencies {
                self.add_modules(dependencies, dotdeploy_config, false)?;
            }
        }
        Ok(())
    }

    /// Determines the filesystem location of a module based on its name.
    ///
    /// # Arguments
    ///
    /// * `module_name` - The name of the module to locate.
    /// * `dotdeploy_config` - The global configuration for dotdeploy.
    ///
    /// # Returns
    ///
    /// A Result containing the PathBuf to the module's location, or an error if it can't be
    /// determined.
    fn locate_module(
        &self,
        module_name: &str,
        dotdeploy_config: &DotdeployConfig,
    ) -> Result<PathBuf> {
        // Determine the path to the module's configuration file
        let path: PathBuf = if module_name.starts_with("hosts") {
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

    /// Creates a temporary module configuration file for testing.
    fn create_temp_module_config(
        dir: &tempfile::TempDir,
        name: &str,
        depends: Option<Vec<&str>>,
    ) -> PathBuf {
        let config = TempModuleConfig {
            depends: depends.map(|deps| deps.into_iter().map(String::from).collect()),
        };

        let path = dir.path().join(name);
        fs::create_dir_all(&path).expect("Failed to create temporary module directory");
        let config_path = path.join("config.toml");
        let config_str = toml::to_string(&config).expect("Failed to serialize temp module config");
        fs::write(&config_path, config_str).expect("Failed to write temp module config");
        path
    }

    /// Creates a DotdeployConfig for testing purposes.
    fn create_test_config(temp_dir: &tempfile::TempDir) -> DotdeployConfig {
        DotdeployConfig {
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
        }
    }

    #[test]
    fn test_add_modules_with_dependencies() -> Result<()> {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let dotdeploy_config = create_test_config(&temp_dir);

        // Create temporary module files with dependencies
        create_temp_module_config(&temp_dir, "module1", Some(vec!["module2"]));
        create_temp_module_config(&temp_dir, "module2", Some(vec!["module3"]));
        create_temp_module_config(&temp_dir, "module3", None);

        let mut queue = ModuleQueue {
            modules: BTreeSet::new(),
            context: BTreeMap::new(),
        };

        // Add the root module, which should recursively add its dependencies
        queue.add_modules(&vec!["module1".to_string()], &dotdeploy_config, true)?;

        // Check that all modules are present in the set
        assert!(queue.modules.iter().any(|m| m.name == "module1"));
        assert!(queue.modules.iter().any(|m| m.name == "module2"));
        assert!(queue.modules.iter().any(|m| m.name == "module3"));

        // Test that adding modules does not cause duplicates
        queue.add_modules(&vec!["module1".to_string()], &dotdeploy_config, true)?;
        assert_eq!(queue.modules.len(), 3);

        Ok(())
    }

    #[test]
    fn test_circular_dependencies() -> Result<()> {
        let temp_dir = tempdir().context("Failed to create temp dir")?;
        let dotdeploy_config = create_test_config(&temp_dir);

        // Create temporary module files with circular dependencies
        create_temp_module_config(&temp_dir, "module1", Some(vec!["module2"]));
        create_temp_module_config(&temp_dir, "module2", Some(vec!["module3"]));
        create_temp_module_config(&temp_dir, "module3", Some(vec!["module1"]));
        create_temp_module_config(&temp_dir, "foo", Some(vec!["module1", "module2", "module3"]));

        let mut queue = ModuleQueue {
            modules: BTreeSet::new(),
            context: BTreeMap::new(),
        };

        // Add the root module, which should handle the circular dependency
        queue.add_modules(&vec!["module1".to_string(), "foo".to_string()], &dotdeploy_config, true)?;

        // Check that all modules are present in the set
        assert!(queue.modules.iter().any(|m| m.name == "module1"));
        assert!(queue.modules.iter().any(|m| m.name == "module2"));
        assert!(queue.modules.iter().any(|m| m.name == "module3"));

        // Ensure that the circular dependency didn't cause infinite recursion
        assert_eq!(queue.modules.len(), 4);

        // Verify that module1 is marked as manual
        let module1 = queue.modules.iter().find(|m| m.name == "module1").unwrap();
        let foo = queue.modules.iter().find(|m| m.name == "foo").unwrap();
        assert_eq!(module1.reason, "manual");
        assert_eq!(foo.reason, "manual");

        // Verify that module2 and module3 are marked as automatic
        let module2 = queue.modules.iter().find(|m| m.name == "module2").unwrap();
        let module3 = queue.modules.iter().find(|m| m.name == "module3").unwrap();
        assert_eq!(module2.reason, "automatic");
        assert_eq!(module3.reason, "automatic");

        Ok(())
    }

    #[test]
    fn test_locate_module() -> Result<()> {
        let temp_dir = tempdir().context("Failed to create temp dir")?;
        let dotdeploy_config = create_test_config(&temp_dir);

        let queue = ModuleQueue {
            modules: BTreeSet::new(),
            context: BTreeMap::new(),
        };

        // Test regular module
        let regular_module = queue.locate_module("test_module", &dotdeploy_config)?;
        assert_eq!(
            regular_module,
            dotdeploy_config.modules_root.join("test_module")
        );

        // Test host-specific module
        let host_module = queue.locate_module("hosts/test_host", &dotdeploy_config)?;
        assert_eq!(host_module, dotdeploy_config.hosts_root.join("test_host"));

        Ok(())
    }
}
