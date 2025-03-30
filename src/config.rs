//! This module handles the configuration for the Dotdeploy application.
//!
//! It provides functionality to read, parse, and initialize the configuration from a TOML file or
//! use default values when necessary.

use color_eyre::Result;
use color_eyre::{eyre::OptionExt, eyre::WrapErr};
use serde::Deserialize;
use std::ffi::OsString;
use std::io::BufRead;
use std::path::{Path, PathBuf};

// -------------------------------------------------------------------------------------------------
// Dotdeploy Config
// -------------------------------------------------------------------------------------------------

/// Representation of the Dotdeploy configuration.
///
/// This struct deserializes the configuration file. The file is expected to be found under
/// `$HOME/.config/dotdeploy/config.toml`.
///
/// # Defaults
///
/// ## Basic Options
/// - `dry_run`: false - Show what would happen without making changes
/// - `force`: false - Skip confirmations for destructive operations
/// - `noconfirm`: false - Assume "yes" instead of prompting
/// - `logs_max`: 15 - Maximum number of logs to retain
///
/// ## Paths
/// - `config_root`: `"$XDG_CONFIG_HOME/dotdeploy"` or `"~/.config/dotdeploy"` - Root folder of
///   dotdeploy configuation
/// - `dotfiles_root`: `"~/.dotfiles/"` - Root folder of dotfiles
/// - `modules_root`: `"~/.dotfiles/modules/"` - Root folder for module declarations
/// - `hosts_root`: `"~/.dotfiles/hosts/"` - Root folder for host declarations
/// - `user_store_path`: `"$XDG_DATA_HOME/dotdeploy"` or `"~/.local/share/dotdeploy"` - User store
///   location
/// - `logs_dir`: `"$XDG_DATA_HOME/dotdeploy/logs"` - Directory for log files
///
/// ## System Detection
/// - `hostname`: Automatically detected from system
/// - `distribution`: Automatically detected from /etc/os-release
///
/// ## Privileges and Permissions
/// - `use_sudo`: true - Use sudo to elevate privileges when needed
/// - `sudo_cmd`: "sudo" - Command used for privilege elevation
/// - `deploy_sys_files`: true - Allow deploying files outside HOME
///
/// ## Package Management
/// - `install_pkg_cmd`: None
/// - `remove_pkg_cmd`: None
/// - `skip_pkg_install`: false - Whether to skip package operations
///
/// # Example Configuration
/// To override options, your `config.toml` might look like this:
///
/// ```toml
/// dotfiles_root = "/path/to/my/dotfiles"
/// modules_root = "/path/to/my/dotfiles/modules"
/// hosts_root = "/path/to/my/dotfiles/hosts"
/// use_sudo = true
/// deploy_sys_files = false
/// ```
#[derive(Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct DotdeployConfig {
    /// Show what would happen without making changes.
    pub(crate) dry_run: bool,
    /// Skip confirmations for destructive operations.
    pub(crate) force: bool,
    /// Assume "yes" instead of prompting
    pub(crate) noconfirm: bool,
    /// Root folder of config.
    #[allow(dead_code)]
    pub(crate) config_file: PathBuf,
    /// Root folder of dotfiles.
    pub(crate) dotfiles_root: PathBuf,
    /// Root folder of Dotedeploy modules. This path stores the module declarations.
    pub(crate) modules_root: PathBuf,
    /// Root folder of Dotedeploy hosts. This path stores the hosts declarations.
    pub(crate) hosts_root: PathBuf,
    /// Host device's hostname.
    pub(crate) hostname: String,
    /// Host device's Linux distribution including version.
    pub(crate) distribution: String,
    /// Use sudo to elevate privileges.
    pub(crate) use_sudo: bool,
    /// Command used for privilege elevation (sudo/doas/etc).
    pub(crate) sudo_cmd: String,
    /// Allow deploying files outside user's HOME directory.
    pub(crate) deploy_sys_files: bool,
    /// Command used to install packages.
    pub(crate) install_pkg_cmd: Option<Vec<OsString>>,
    /// Command used to remove packages.
    pub(crate) remove_pkg_cmd: Option<Vec<OsString>>,
    /// Skip package installation during deployment
    pub(crate) skip_pkg_install: bool,
    /// Directory of the user store
    pub(crate) user_store_path: PathBuf,
    /// Directory of the log files
    pub(crate) logs_dir: PathBuf,
    /// Maximum number of log files to retain
    pub(crate) logs_max: usize,
}

// -------------------------------------------------------------------------------------------------
// Config Builder
// -------------------------------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DotdeployConfigBuilderIntermediate {
    pub(crate) dry_run: Option<bool>,
    pub(crate) force: Option<bool>,
    pub(crate) noconfirm: Option<bool>,
    pub(crate) config_file: Option<PathBuf>,
    pub(crate) dotfiles_root: Option<PathBuf>,
    pub(crate) modules_root: Option<PathBuf>,
    pub(crate) hosts_root: Option<PathBuf>,
    pub(crate) hostname: Option<String>,
    pub(crate) distribution: Option<String>,
    pub(crate) use_sudo: Option<bool>,
    pub(crate) sudo_cmd: Option<String>,
    pub(crate) deploy_sys_files: Option<bool>,
    pub(crate) install_pkg_cmd: Option<Vec<String>>,
    pub(crate) remove_pkg_cmd: Option<Vec<String>>,
    pub(crate) skip_pkg_install: Option<bool>,
    pub(crate) user_store_path: Option<PathBuf>,
    pub(crate) logs_dir: Option<PathBuf>,
    pub(crate) logs_max: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(from = "DotdeployConfigBuilderIntermediate")]
pub(crate) struct DotdeployConfigBuilder {
    pub(crate) dry_run: Option<bool>,
    pub(crate) force: Option<bool>,
    pub(crate) noconfirm: Option<bool>,
    pub(crate) config_file: Option<PathBuf>,
    pub(crate) dotfiles_root: Option<PathBuf>,
    pub(crate) modules_root: Option<PathBuf>,
    pub(crate) hosts_root: Option<PathBuf>,
    pub(crate) hostname: Option<String>,
    pub(crate) distribution: Option<String>,
    pub(crate) use_sudo: Option<bool>,
    pub(crate) sudo_cmd: Option<String>,
    pub(crate) deploy_sys_files: Option<bool>,
    pub(crate) install_pkg_cmd: Option<Vec<OsString>>,
    pub(crate) remove_pkg_cmd: Option<Vec<OsString>>,
    pub(crate) skip_pkg_install: Option<bool>,
    pub(crate) user_store_path: Option<PathBuf>,
    pub(crate) logs_dir: Option<PathBuf>,
    pub(crate) logs_max: Option<usize>,
}

impl From<DotdeployConfigBuilderIntermediate> for DotdeployConfigBuilder {
    fn from(intermediate: DotdeployConfigBuilderIntermediate) -> Self {
        Self {
            dry_run: intermediate.dry_run,
            force: intermediate.force,
            noconfirm: intermediate.noconfirm,
            config_file: intermediate.config_file,
            dotfiles_root: intermediate.dotfiles_root,
            modules_root: intermediate.modules_root,
            hosts_root: intermediate.hosts_root,
            hostname: intermediate.hostname,
            distribution: intermediate.distribution,
            use_sudo: intermediate.use_sudo,
            sudo_cmd: intermediate.sudo_cmd,
            deploy_sys_files: intermediate.deploy_sys_files,
            install_pkg_cmd: intermediate
                .install_pkg_cmd
                .map(|v| v.into_iter().map(OsString::from).collect()),
            remove_pkg_cmd: intermediate
                .remove_pkg_cmd
                .map(|v| v.into_iter().map(OsString::from).collect()),
            skip_pkg_install: intermediate.skip_pkg_install,
            user_store_path: intermediate.user_store_path,
            logs_dir: intermediate.logs_dir,
            logs_max: intermediate.logs_max,
        }
    }
}

impl DotdeployConfigBuilder {
    // --
    // * Builders

    pub(crate) fn with_dry_run(&mut self, dry_run: Option<bool>) -> &mut Self {
        let new = self;
        new.dry_run = dry_run;
        new
    }

    pub(crate) fn with_force(&mut self, force: Option<bool>) -> &mut Self {
        let new = self;
        new.force = force;
        new
    }

    pub(crate) fn with_noconfirm(&mut self, noconfirm: Option<bool>) -> &mut Self {
        let new = self;
        new.noconfirm = noconfirm;
        new
    }

    pub(crate) fn with_config_file(&mut self, config_file: Option<PathBuf>) -> &mut Self {
        let new = self;
        new.config_file = config_file;
        new
    }

    pub(crate) fn with_dotfiles_root(&mut self, dotfiles_root: Option<PathBuf>) -> &mut Self {
        let new = self;
        new.dotfiles_root = dotfiles_root;
        new
    }

    pub(crate) fn with_modules_root(&mut self, modules_root: Option<PathBuf>) -> &mut Self {
        let new = self;
        new.modules_root = modules_root;
        new
    }

    pub(crate) fn with_hosts_root(&mut self, hosts_root: Option<PathBuf>) -> &mut Self {
        let new = self;
        new.hosts_root = hosts_root;
        new
    }

    pub(crate) fn with_hostname(&mut self, hostname: Option<String>) -> &mut Self {
        let new = self;
        new.hostname = hostname;
        new
    }

    pub(crate) fn with_distribution(&mut self, distribution: Option<String>) -> &mut Self {
        let new = self;
        new.distribution = distribution;
        new
    }

    pub(crate) fn with_use_sudo(&mut self, use_sudo: Option<bool>) -> &mut Self {
        let new = self;
        new.use_sudo = use_sudo;
        new
    }

    pub(crate) fn with_sudo_cmd(&mut self, sudo_cmd: Option<String>) -> &mut Self {
        let new = self;
        new.sudo_cmd = sudo_cmd;
        new
    }

    pub(crate) fn with_deploy_sys_files(&mut self, deploy_sys_files: Option<bool>) -> &mut Self {
        let new = self;
        new.deploy_sys_files = deploy_sys_files;
        new
    }

    pub(crate) fn with_install_pkg_cmd(
        &mut self,
        install_pkg_cmd: Option<Vec<OsString>>,
    ) -> &mut Self {
        let new = self;
        new.install_pkg_cmd = install_pkg_cmd;
        new
    }

    pub(crate) fn with_remove_pkg_cmd(
        &mut self,
        remove_pkg_cmd: Option<Vec<OsString>>,
    ) -> &mut Self {
        let new = self;
        new.remove_pkg_cmd = remove_pkg_cmd;
        new
    }

    pub(crate) fn with_skip_pkg_install(&mut self, skip_pkg_install: Option<bool>) -> &mut Self {
        let new = self;
        new.skip_pkg_install = skip_pkg_install;
        new
    }

    pub(crate) fn with_user_store_path(&mut self, user_store_path: Option<PathBuf>) -> &mut Self {
        let new = self;
        new.user_store_path = user_store_path;
        new
    }

    pub(crate) fn with_logs_dir(&mut self, logs_dir: Option<PathBuf>) -> &mut Self {
        let new = self;
        new.logs_dir = logs_dir;
        new
    }

    pub(crate) fn with_logs_max(&mut self, logs_max: Option<usize>) -> &mut Self {
        let new = self;
        new.logs_max = logs_max;
        new
    }

    /// Reads and returns the contents of a configuration file.
    ///
    /// Takes a path to a TOML configuration file and returns its contents as a string. This is a
    /// helper method used during the build process to load configuration from disk.
    ///
    /// # Arguments
    /// * `path` - Path to the configuration file to read
    ///
    /// # Errors
    /// Returns an error if:
    /// * The file cannot be read
    /// * The file path is invalid
    /// * Permission is denied
    fn read_config_file(&self, path: &Path) -> Result<String> {
        // Read and return the contents of the config file
        let config_file_content: String = std::fs::read_to_string(&path)
            .wrap_err_with(|| format!("Failed to read config from {}", path.display()))?;

        Ok(config_file_content)
    }

    /// Helper function to expand a path from configuration
    fn expand_config_path<F>(
        value: &Option<PathBuf>,
        parsed_value: &Option<PathBuf>,
        default_fn: F,
    ) -> Result<PathBuf>
    where
        F: FnOnce() -> Result<PathBuf>,
    {
        match value {
            Some(path) => crate::utils::file_fs::expand_path::<&PathBuf, &str>(path, None),
            None => parsed_value.as_ref().map_or_else(default_fn, |p| {
                crate::utils::file_fs::expand_path::<&PathBuf, &str>(p, None)
            }),
        }
    }

    /// Constructs the final configuration by merging defaults, file values, and runtime overrides
    ///
    /// Resolution order (highest priority last):
    /// 1. Default values
    /// 2. Config file values
    /// 3. Explicit builder overrides
    ///
    /// Expands all paths and handles environment variables. Automatically detects hostname and
    /// distribution if not explicitly set.
    pub(crate) fn build(&self, verbosity: u8) -> Result<DotdeployConfig> {
        // Determine the config file path based on environment variables
        let config_file_path = if let Some(ref path) = self.config_file {
            Clone::clone(path)
        } else {
            dirs::config_dir()
                .ok_or_eyre("Could not determine user's config directory")?
                .join("dotdeploy")
                .join("config.toml")
        };

        // Try to read config file, use empty string if not found
        let conf_string = match self.read_config_file(&config_file_path) {
            Ok(s) => s,
            Err(_) => {
                if verbosity > 0 {
                    eprintln!("No config file found in {}", &config_file_path.display());
                    eprintln!("Default config values will be used")
                }
                "".to_string()
            }
        };
        let parsed_data: DotdeployConfigBuilder = toml::from_str(&conf_string)?;

        // Define constants for default paths
        const DEFAULT_DOTFILES_DIR: &str = "~/.dotfiles";
        const DEFAULT_MODULES_DIR: &str = "modules";
        const DEFAULT_HOSTS_DIR: &str = "hosts";

        let dotfiles_root =
            Self::expand_config_path(&self.dotfiles_root, &parsed_data.dotfiles_root, || {
                crate::utils::file_fs::expand_path::<&str, &str>(DEFAULT_DOTFILES_DIR, None)
            })?;

        let modules_root =
            Self::expand_config_path(&self.modules_root, &parsed_data.modules_root, || {
                crate::utils::file_fs::expand_path::<PathBuf, &str>(
                    dotfiles_root.join(DEFAULT_MODULES_DIR),
                    None,
                )
            })?;

        let hosts_root =
            Self::expand_config_path(&self.hosts_root, &parsed_data.hosts_root, || {
                crate::utils::file_fs::expand_path::<PathBuf, &str>(
                    dotfiles_root.join(DEFAULT_HOSTS_DIR),
                    None,
                )
            })?;

        let user_store_path =
            Self::expand_config_path(&self.user_store_path, &parsed_data.user_store_path, || {
                crate::utils::file_fs::expand_path::<PathBuf, &str>(
                    dirs::data_dir()
                        .ok_or_eyre("Could not determine user's data directory")?
                        .join("dotdeploy"),
                    None,
                )
            })?;

        Ok(DotdeployConfig {
            dry_run: self.dry_run.unwrap_or(parsed_data.dry_run.unwrap_or(false)),
            force: self.force.unwrap_or(parsed_data.force.unwrap_or(false)),
            noconfirm: self
                .noconfirm
                .unwrap_or(parsed_data.noconfirm.unwrap_or(false)),
            config_file: config_file_path,
            dotfiles_root,
            modules_root,
            hosts_root,
            hostname: match self.hostname {
                Some(ref value) => Clone::clone(value),
                None => parsed_data
                    .hostname
                    .unwrap_or_else(|| get_hostname(verbosity).unwrap()),
            },
            distribution: match self.distribution {
                Some(ref value) => Clone::clone(value),
                None => parsed_data
                    .distribution
                    .unwrap_or_else(|| get_distro(verbosity).unwrap()),
            },
            use_sudo: self
                .use_sudo
                .unwrap_or(parsed_data.use_sudo.unwrap_or(true)),
            sudo_cmd: match self.sudo_cmd {
                Some(ref value) => Clone::clone(value),
                None => parsed_data.sudo_cmd.unwrap_or_else(|| "sudo".to_string()),
            },
            deploy_sys_files: self
                .deploy_sys_files
                .unwrap_or(parsed_data.deploy_sys_files.unwrap_or(true)),
            install_pkg_cmd: match self.install_pkg_cmd {
                Some(ref value) => Some(Clone::clone(value)),
                None => parsed_data.install_pkg_cmd,
            },
            remove_pkg_cmd: match self.remove_pkg_cmd {
                Some(ref value) => Some(Clone::clone(value)),
                None => parsed_data.remove_pkg_cmd,
            },
            skip_pkg_install: self
                .skip_pkg_install
                .unwrap_or(parsed_data.skip_pkg_install.unwrap_or(false)),
            user_store_path,
            logs_dir: match self.logs_dir {
                Some(ref value) => Clone::clone(value),
                None => crate::logs::get_default_log_dir()?,
            },
            logs_max: self.logs_max.unwrap_or(15),
        })
    }
}

/// Retrieve the host's distribution.
///
/// Checks `/etc/os-release` for the "ID" field and retrieves the value. Returns "unknown" if
/// not successful.
fn get_distro(verbosity: u8) -> Result<String> {
    let os_release_file =
        std::fs::File::open("/etc/os-release").wrap_err("Failed to open '/etc/os-release'");
    let mut distro_string = String::new();
    let mut distro_version_string = String::new();
    match os_release_file {
        Ok(file) => {
            let reader = std::io::BufReader::new(file);
            // Iterate through lines of /etc/os-release
            for line in reader.lines().map_while(Result::ok) {
                if line.starts_with("ID=") {
                    // Extract and return the distribution ID
                    distro_string.push_str(line.trim_start_matches("ID=").trim_matches('"').trim());
                } else if line.starts_with("VERSION_ID=") {
                    // Extract and return the distribution version ID
                    distro_version_string.push_str(
                        line.trim_start_matches("VERSION_ID=")
                            .trim_matches('"')
                            .trim(),
                    );
                }
            }
            // If ID field is not found, return "unknown"
            if distro_string.is_empty() {
                distro_string.push_str("unknown");
            }
        }
        Err(e) => {
            if verbosity > 0 {
                eprintln!("Could not open '/etc/os-release, defaulting to 'unknown'\n{e:?}")
            }
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
fn get_hostname(verbosity: u8) -> Result<String> {
    match nix::unistd::gethostname() {
        Ok(hostname) => match hostname.into_string() {
            Ok(host) => Ok(host),
            Err(e) => {
                if verbosity > 0 {
                    eprintln!("Could not determine hostname, defaulting to 'unknown'.\n{e:?}")
                }
                Ok("unknown".to_string())
            }
        },
        Err(e) => {
            if verbosity > 0 {
                eprintln!("Could not determine hostname, defaulting to 'unknown'.\n{e:?}")
            }
            Ok("unknown".to_string())
        }
    }
}

// -------------------------------------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;
    use tempfile::TempDir;

    /// Test configuration struct used in unit tests
    #[derive(Serialize)]
    struct TestConf {
        dotfiles_root: Option<String>,
        modules_root: Option<String>,
        hosts_root: Option<String>,
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

        temp_env::with_var("HOME", Some(temp_dir.path()), || -> Result<()> {
            let test_config = DotdeployConfigBuilder::default()
                .with_config_file(Some(PathBuf::from(temp_dir.path())))
                .build(0)?;

            assert_eq!(
                test_config.dotfiles_root,
                temp_dir.path().join(".dotfiles"),
                "Default dotfiles_root should be ~/.dotfiles"
            );
            assert_eq!(
                test_config.modules_root,
                temp_dir.path().join(".dotfiles").join("modules"),
                "Default modules_root should be ~/.dotfiles/modules"
            );
            assert_eq!(
                test_config.hosts_root,
                temp_dir.path().join(".dotfiles").join("hosts"),
                "Default hosts_root should be ~/.dotfiles/hosts"
            );

            assert!(
                !test_config.distribution.is_empty(),
                "Should detect distribution from OS or fallback to 'unknown'"
            );
            assert!(
                !test_config.hostname.is_empty(),
                "Should detect hostname via gethostname() or fallback to 'unknown'"
            );
            assert!(test_config.use_sudo, "use_sudo should default to true");
            assert!(
                test_config.deploy_sys_files,
                "deploy_sys_files should default to true"
            );
            Ok(())
        })?;

        Ok(())
    }

    #[test]
    fn test_create_config_with_file() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let test_config_content = TestConf {
            dotfiles_root: Some("/tmp".to_string()),
            modules_root: None,
            hosts_root: None,
        };
        create_config_file(&temp_dir, &test_config_content)?;
        let test_config = DotdeployConfigBuilder::default()
            .with_config_file(Some(temp_dir.into_path().join("dotdeploy")))
            .build(0)?;

        assert_eq!(
            test_config.dotfiles_root,
            PathBuf::from("/tmp"),
            "dotfiles_root should be set from config file"
        );
        assert_eq!(
            test_config.modules_root,
            PathBuf::from("/tmp/modules"),
            "modules_root should default to dotfiles_root/modules"
        );
        assert_eq!(
            test_config.hosts_root,
            PathBuf::from("/tmp/hosts"),
            "hosts_root should default to dotfiles_root/hosts"
        );

        Ok(())
    }

    #[test]
    fn test_create_config_with_custom_paths() -> Result<()> {
        let temp_dir = TempDir::new()?;

        let test_config_content = TestConf {
            dotfiles_root: Some("/foo".to_string()),
            modules_root: Some("/bar".to_string()),
            hosts_root: Some("/baz".to_string()),
        };

        create_config_file(&temp_dir, &test_config_content)?;

        let test_config = DotdeployConfigBuilder::default()
            .with_config_file(Some(temp_dir.into_path().join("dotdeploy")))
            .build(0)?;

        assert_eq!(
            test_config.dotfiles_root,
            PathBuf::from("/foo"),
            "dotfiles_root should be set from config file"
        );
        assert_eq!(
            test_config.modules_root,
            PathBuf::from("/bar"),
            "modules_root should be set from config file"
        );
        assert_eq!(
            test_config.hosts_root,
            PathBuf::from("/baz"),
            "hosts_root should be set from config file"
        );

        Ok(())
    }
}
