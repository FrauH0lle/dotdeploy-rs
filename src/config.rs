//! This module handles the configuration for the Dotdeploy application.
//!
//! It provides functionality to read, parse, and initialize the configuration from a TOML file or
//! use default values when necessary.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::env;
use std::io::BufRead;
use std::path::PathBuf;

// Update the Defaults section in the docstring below. AI!
/// Representation of the Dotdeploy configuration.
///
/// This struct deserializes the configuration file. The file is expected to be found under
/// `$HOME/.config/dotdeploy/config.toml`.
///
/// # Defaults
///
/// - `dry_run`: false
/// - `force`: false
/// - `noconfirm`: false
/// - `config_root`: `"~/.dotfiles/"`
/// - `modules_root`: `"~/.dotfiles/modules/"`
/// - `hosts_root`: `"~/.dotfiles/hosts/"`
/// - `hostname`: Automatically detected by default if possible.
/// - `distribution`: Automatically detected by default if possible.
/// - `use_sudo`: true
/// - `deploy_sys_files`: true
/// - `install_pkg_cmd`: None. Will choose appropriate commands for supported distributions.
/// - `remove_pkg_cmd`: None. Will choose appropriate commands for supported distributions.
/// - `skip_pkg_install`: false
/// - `user_store_path`: `"$XDG_DATA_HOME/dotdeploy"` or `"~/.local/share/dotdeploy"`
/// - `system_store_path`: `"/var/lib/dotdeploy"`
///
/// # Example Configuration
/// To override options, your `config.toml` might look like this:
///
/// ```toml
/// config_root = "/path/to/my/dotfiles"
/// modules_root = "/path/to/my/dotfiles/modules"
/// hosts_root = "/path/to/my/dotfiles/hosts"
/// use_sudo = true
/// deploy_sys_files = false
/// ```
#[derive(Deserialize, Debug)]
pub(crate) struct DotdeployConfig {
    /// Show what would happen without making changes.
    pub(crate) dry_run: bool,
    /// Skip confirmations for destructive operations.
    pub(crate) force: bool,
    /// Assume "yes" instead of prompting
    pub(crate) noconfirm: bool,
    /// Root folder of dotfiles.
    pub(crate) config_root: PathBuf,
    /// Root folder of Dotedeploy modules. This path stores the module declarations.
    pub(crate) modules_root: PathBuf,
    /// Root folder of Dotedeploy hosts. This path stores the hosts declarations.
    pub(crate) hosts_root: PathBuf,
    /// Host device's hostname.
    pub(crate) hostname: String,
    /// Host device's Linux distribution.
    pub(crate) distribution: String,
    /// Use sudo to elevate privileges.
    pub(crate) use_sudo: bool,
    /// Use sudo to elevate privileges.
    pub(crate) sudo_cmd: String,
    /// Deploy files to directories other than the user's HOME.
    pub(crate) deploy_sys_files: bool,
    /// Command used to install packages.
    pub(crate) install_pkg_cmd: Option<Vec<String>>,
    /// Command used to remove packages.
    pub(crate) remove_pkg_cmd: Option<Vec<String>>,
    /// Skip package installation during deployment
    pub(crate) skip_pkg_install: bool,
    /// Directory of the user store
    pub(crate) user_store_path: PathBuf,
    /// Directory of the system store
    pub(crate) system_store_path: PathBuf,
}

impl DotdeployConfig {
    /// Builds the path to the dotdeploy config file based on environment variables.
    ///
    /// Checks `XDG_CONFIG_HOME` first and then `HOME`. Returns the path to the config file as a
    /// `String`.
    ///
    /// # Errors
    /// Returns an error if reading the config file fails.
    fn read_config_file() -> Result<String> {
        // Determine the config file path based on environment variables
        let config_file_path: PathBuf = if let Ok(xdg_dir) = env::var("XDG_CONFIG_HOME") {
            [xdg_dir.as_str(), "dotdeploy"].iter().collect()
        } else if let Ok(home_dir) = env::var("HOME") {
            [home_dir.as_str(), ".config", "dotdeploy"].iter().collect()
        } else if let Ok(user_name) = env::var("USER") {
            ["/home", user_name.as_str(), ".config", "dotdeploy"]
                .iter()
                .collect()
        } else {
            debug!("Could not determine config file path. Using current working directory.");
            PathBuf::from(".")
        };

        // Construct the full path to the config file
        let config_file: PathBuf = config_file_path.join("config.toml");

        // Read and return the contents of the config file
        let config_file_content: String = std::fs::read_to_string(&config_file)
            .with_context(|| format!("Failed to read config from {:?}", &config_file))?;

        Ok(config_file_content)
    }

    /// Retrieve the host's distribution.
    ///
    /// Checks `/etc/os-release` for the "ID" field and retrieves the value. Returns "unknown" if
    /// not successful.
    fn get_distro() -> Result<String> {
        let os_release_file = std::fs::File::open("/etc/os-release");
        let mut distro_string = String::new();
        let mut distro_version_string = String::new();
        match os_release_file {
            Ok(file) => {
                let reader = std::io::BufReader::new(file);
                // Iterate through lines of /etc/os-release
                for line in reader.lines() {
                    if let Ok(line) = line {
                        if line.starts_with("ID=") {
                            // Extract and return the distribution ID
                            distro_string.push_str(line.trim_start_matches("ID=").trim());
                        } else if line.starts_with("VERSION_ID=") {
                            // Extract and return the distribution version ID
                            distro_version_string
                                .push_str(line.trim_start_matches("VERSION_ID=").trim());
                        }
                    }
                }
                // If ID field is not found, return "unknown"
                if distro_string.is_empty() {
                    distro_string.push_str("unknown");
                }
            }
            Err(e) => {
                error!("Could not open '/etc/os-release, defaulting to 'unknown'\n{e:?}");
                distro_string.push_str("unknown");
            }
        }
        if !distro_version_string.is_empty() {
            Ok(format!("{distro_string}:{distro_version_string}"))
        } else {
            Ok(distro_string)
        }
    }

    /// Retrieve the hostname.
    ///
    /// Uses the `nix` crate to get the system hostname. Returns "unknown" if not successful.
    fn get_hostname() -> Result<String> {
        match nix::unistd::gethostname() {
            Ok(hostname) => match hostname.into_string() {
                Ok(host) => Ok(host),
                Err(e) => {
                    error!(
                        "Could not determine hostname, defaulting to 'unknown'.\n {:?}",
                        e
                    );
                    Ok("unknown".to_string())
                }
            },
            Err(e) => {
                error!(
                    "Could not determine hostname, defaulting to 'unknown'.\n {:?}",
                    e
                );
                Ok("unknown".to_string())
            }
        }
    }

    /// Initialize the [DotdeployConfig] struct.
    ///
    /// If found, it parses the config file and tries to expand all paths. If the config file is
    /// absent or fields are missing it will use default values (see [DotdeployConfig]).
    pub(crate) fn init() -> Result<DotdeployConfig> {
        // Attempt to read the config file, use an empty string if not found
        let conf_string = match Self::read_config_file() {
            Ok(s) => s,
            Err(e) => {
                debug!("{:?}", e);
                debug!("Default config values will be used");
                "".to_string()
            }
        };

        // Intermediate struct for the parsed config file data
        #[derive(Deserialize)]
        struct ParsedFile {
            dry_run: Option<bool>,
            force: Option<bool>,
            config_root: Option<String>,
            modules_root: Option<String>,
            hosts_root: Option<String>,
            hostname: Option<String>,
            distribution: Option<String>,
            use_sudo: Option<bool>,
            sudo_cmd: Option<String>,
            deploy_sys_files: Option<bool>,
            install_pkg_cmd: Option<Vec<String>>,
            remove_pkg_cmd: Option<Vec<String>>,
            skip_pkg_install: Option<bool>,
            noconfirm: Option<bool>,
            user_store_path: Option<PathBuf>,
            system_store_path: Option<PathBuf>,
        }

        // Parse the configuration string
        let parsed_data: ParsedFile = toml::from_str(&conf_string)?;

        // Set config_root to ~/.dotfiles if empty
        let config_root = parsed_data
            .config_root
            .map(|path| {
                shellexpand::full(&path)
                    .context("Failed to expand file path")
                    .unwrap()
                    .to_string()
            })
            .unwrap_or_else(|| shellexpand::full("~/.dotfiles").unwrap().to_string());

        // Set modules_root based on config_root if not already set
        let modules_root = parsed_data
            .modules_root
            .map(|path| {
                shellexpand::full(&path)
                    .context("Failed to expand file path")
                    .unwrap()
                    .to_string()
            })
            .unwrap_or_else(|| {
                PathBuf::from(&config_root)
                    .join("modules")
                    .to_string_lossy()
                    .to_string()
            });

        // Set hosts_root based on config_root if not already set
        let hosts_root = parsed_data
            .hosts_root
            .map(|path| {
                shellexpand::full(&path)
                    .context("Failed to expand file path")
                    .unwrap()
                    .to_string()
            })
            .unwrap_or_else(|| {
                PathBuf::from(&config_root)
                    .join("hosts")
                    .to_string_lossy()
                    .to_string()
            });

        // Construct and return the final DotdeployConfig struct
        Ok(DotdeployConfig {
            dry_run: parsed_data.dry_run.unwrap_or(false),
            force: parsed_data.force.unwrap_or(false),
            config_root: PathBuf::from(config_root),
            modules_root: PathBuf::from(modules_root),
            hosts_root: PathBuf::from(hosts_root),
            distribution: parsed_data
                .distribution
                .unwrap_or_else(|| Self::get_distro().unwrap()),
            hostname: parsed_data
                .hostname
                .unwrap_or_else(|| Self::get_hostname().unwrap()),
            use_sudo: parsed_data.use_sudo.unwrap_or(true),
            sudo_cmd: parsed_data.sudo_cmd.unwrap_or("sudo".to_string()),
            deploy_sys_files: parsed_data.deploy_sys_files.unwrap_or(true),
            install_pkg_cmd: parsed_data.install_pkg_cmd,
            remove_pkg_cmd: parsed_data.remove_pkg_cmd,
            skip_pkg_install: parsed_data.skip_pkg_install.unwrap_or(false),
            noconfirm: parsed_data.noconfirm.unwrap_or(false),
            user_store_path: parsed_data.user_store_path.unwrap_or_else(|| {
                if let Ok(xdg_dir) = env::var("XDG_DATA_HOME") {
                    // Use XDG_DATA_HOME if available
                    [xdg_dir.as_str(), "dotdeploy"].iter().collect()
                } else {
                    // Fallback to HOME/.local/share/dotdeploy
                    [
                        env::var("HOME")
                            .expect("HOME environment variable not set")
                            .as_str(),
                        ".local",
                        "share",
                        "dotdeploy",
                    ]
                    .iter()
                    .collect()
                }
            }),
            system_store_path: parsed_data
                .system_store_path
                .unwrap_or(PathBuf::from("/var/lib/dotdeploy")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;
    use tempfile::TempDir;

    /// Test configuration struct used in unit tests
    #[derive(Serialize)]
    struct TestConf {
        config_root: Option<String>,
        modules_root: Option<String>,
        hosts_root: Option<String>,
    }

    struct EnvGuard<'a> {
        key: &'a str,
        original: Option<String>,
    }

    impl Drop for EnvGuard<'_> {
        fn drop(&mut self) {
            match &self.original {
                Some(val) => env::set_var(self.key, val),
                None => env::remove_var(self.key),
            }
        }
    }

    /// Helper function to set an environment variable and return a guard that will restore the
    /// original value when dropped.
    fn set_env_var<'a>(key: &'a str, value: &str) -> EnvGuard<'a> {
        let original = env::var(key).ok();
        env::set_var(key, value);

        EnvGuard { key, original }
    }

    /// Helper function to create a config file in a temporary directory
    fn create_config_file(dir: &TempDir, config: &TestConf) -> Result<()> {
        let config_dir = dir.path().join("dotdeploy");
        std::fs::create_dir_all(&config_dir)?;
        let config_file = config_dir.join("config.toml");
        std::fs::write(config_file, toml::to_string(config)?)?;
        Ok(())
    }

    #[test]
    fn test_create_config_no_file() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let _guard = set_env_var("XDG_CONFIG_HOME", temp_dir.path().to_str().unwrap());

        assert_eq!(
            env::var("XDG_CONFIG_HOME"),
            Ok(temp_dir.path().to_string_lossy().to_string())
        );

        let conf = DotdeployConfig::init()?;

        assert_eq!(
            conf.config_root,
            PathBuf::from(shellexpand::full("~/.dotfiles").unwrap().to_string())
        );
        assert_eq!(
            conf.modules_root,
            PathBuf::from(
                shellexpand::full("~/.dotfiles/modules")
                    .unwrap()
                    .to_string()
            )
        );
        assert_eq!(
            conf.hosts_root,
            PathBuf::from(shellexpand::full("~/.dotfiles/hosts").unwrap().to_string())
        );

        assert!(!conf.distribution.is_empty());
        assert!(!conf.hostname.is_empty());
        assert!(conf.use_sudo);
        assert!(conf.deploy_sys_files);

        Ok(())
    }

    #[test]
    fn test_create_config_with_file() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let _guard = set_env_var("XDG_CONFIG_HOME", temp_dir.path().to_str().unwrap());

        let test_config = TestConf {
            config_root: Some("/tmp".to_string()),
            modules_root: None,
            hosts_root: None,
        };
        create_config_file(&temp_dir, &test_config)?;

        let conf = DotdeployConfig::init()?;

        assert_eq!(conf.config_root, PathBuf::from("/tmp"));
        assert_eq!(conf.modules_root, PathBuf::from("/tmp/modules"));
        assert_eq!(conf.hosts_root, PathBuf::from("/tmp/hosts"));

        Ok(())
    }

    #[test]
    fn test_create_config_with_custom_paths() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let _guard = set_env_var("XDG_CONFIG_HOME", temp_dir.path().to_str().unwrap());

        let test_config = TestConf {
            config_root: Some("/foo".to_string()),
            modules_root: Some("/bar".to_string()),
            hosts_root: Some("/baz".to_string()),
        };
        create_config_file(&temp_dir, &test_config)?;

        let conf = DotdeployConfig::init()?;
        assert_eq!(conf.config_root, PathBuf::from("/foo"));
        assert_eq!(conf.modules_root, PathBuf::from("/bar"));
        assert_eq!(conf.hosts_root, PathBuf::from("/baz"));

        Ok(())
    }
}
