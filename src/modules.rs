use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::read_module;

/// Dotdeploy module
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Module {
    /// Module name
    pub(crate) name: String,
    /// The location/path of the module
    pub(crate) location: PathBuf,
    /// Reason why a module is added
    pub(crate) reason: String,
    /// Module configuration,
    pub(crate) config: read_module::ModuleConfig,
}

/// Adds a module to the given BTreeSet of modules, processing its dependencies recursively.
///
/// This function attempts to read the module's configuration from a specified path based on whether
/// the module is categorized under "hosts" or is a regular module. If the module's configuration is
/// successfully read, the function then recursively processes any dependencies specified within the
/// configuration, adding them to the set of modules before finally adding the module itself.
///
/// # Parameters
///
/// - `module_name`: The name of the module to be added. This name is used to determine the path
///   to the module's configuration file.
/// - `dotdeploy_config`: Reference to the global configuration of the dotdeploy, which contains
///   paths to the root directories for hosts and modules.
/// - `modules`: A mutable reference to a BTreeSet of `Module` instances where the module (and
///   any of its dependencies) will be inserted.
///
/// # Errors
///
/// - The function logs a error message if the module's configuration cannot be read but will try to
///   continue.
pub(crate) fn add_module(
    module_name: &str,
    dotdeploy_config: &crate::config::ConfigFile,
    modules: &mut std::collections::BTreeSet<Module>,
    manual: bool,
    context: &mut std::collections::BTreeMap<String, String>,
) -> Result<()> {
    debug!("Processing module: {}", module_name);
    // Export module name

    // Determine the path to the module's configuration file based on its type (host or regular
    // module).
    let path: PathBuf = if module_name.starts_with("hosts") {
        dotdeploy_config
            .hosts_root
            .join(module_name.trim_start_matches("hosts/"))
    } else {
        dotdeploy_config.modules_root.join(module_name)
    };

    // Export current module path
    std::env::set_var("DOD_CURRENT_MODULE", &path);

    // Attempt to read the module's configuration.
    match read_module::ModuleConfig::read_config(&path) {
        Ok(config) => {
            // Extract context vars
            if let Some(ref vars) = config.context_vars {
                for (k, v) in vars.iter() {
                    context.insert(k.to_string(), v.to_string());
                }
            }

            // If dependencies are specified, process each recursively.
            if let Some(dependencies) = &config.depends {
                for dependency in dependencies {
                    add_module(&dependency, dotdeploy_config, modules, false, context)?;
                }
            }

            // Insert the module into the set.
            modules.insert(Module {
                name: module_name.to_string(),
                location: path,
                config,
                reason: match manual {
                    true => "manual".to_string(),
                    false => "automatic".to_string(),
                },
            });
            Ok(())
        }
        // Error if the module's configuration cannot be read.
        Err(e) => Err(e)
            .with_context(|| format!("Failed to read module configuration for {}", module_name)),
    }
}

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
