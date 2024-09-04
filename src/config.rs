use anyhow::{Context, Result};
use serde::Deserialize;
use std::env;
use std::io::BufRead;
use std::path::PathBuf;
use std::collections::VecDeque;

/// Representation of the Dotdeploy configuration.
///
/// This struct deserializes the configuration file. The file is expected to be found under
/// `$HOME/.config/dotdeploy/config.toml`.
///
/// # Defaults
/// - `config_root`: The root folder where the dotfiles are located.
///   Defaults to `"~/.dotfiles/"` if not specified.
/// - `modules_root`: The folder under `config_root` where modules are located.
///   Defaults to `"~/.dotfiles/modules/"` if not specified.
/// - `hosts_root`: The folder under `config_root` where host-specific configurations are stored.
///   Defaults to `"~/.dotfiles/hosts/"` if not specified.
/// - `hostname`: Device's hostname.
///   Automatically detected by default if possible.
/// - `distribution`: Device's Linux distribution.
///   Automatically detected by default if possible.
///
/// These paths can be overridden in the Dotdeploy configuration.
///
/// # Example Configuration
/// To override these paths, your `config.toml` might look like this:
///
/// ```toml
/// config_root = "/path/to/my/dotfiles"
/// modules_root = "/path/to/my/dotfiles/modules"
/// hosts_root = "/path/to/my/dotfiles/hosts"
/// ```
#[derive(Deserialize, Debug)]
pub(crate) struct ConfigFile {
    /// Root folder of dotfiles.
    pub(crate) config_root: PathBuf,
    /// Root folder of modules. This path stores the module declarations.
    pub(crate) modules_root: PathBuf,
    /// Root folder of hosts. This path stores the hosts declarations.
    pub(crate) hosts_root: PathBuf,
    /// Host device’s hostname.
    pub(crate) hostname: String,
    /// Host device’s Linux distribution.
    pub(crate) distribution: String,
    /// Use sudo to elevate privileges.
    pub(crate) use_sudo: bool,
    /// Deploy files to directories other than the user's HOME.
    pub(crate) deploy_sys_files: bool,
    /// Command used to install packages.
    pub(crate) intall_pkg_cmd: Option<VecDeque<String>>,
    /// Command used to remove packages.
    pub(crate) remove_pkg_cmd: Option<VecDeque<String>>,
    /// Skip package installation during deployment
    pub(crate) skip_pkg_install: bool,

}

impl ConfigFile {
    /// Builds the path to the dotdeploy config file based on environment variables.
    ///
    /// Checks `XDG_CONFIG_HOME` first and then `HOME`. Returns the path to the config file as a
    /// `String`.
    ///
    /// # Errors
    /// Returns an error if reading the config file fails.
    fn read_config_file() -> Result<String> {
        // Build path to dotdeploy config file. Check if XDG_CONFIG_HOME is set, else use HOME or
        // USER to build the path.
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

        // Read config file
        let config_file: PathBuf = config_file_path.join("config.toml");
        let config_file_content: String = std::fs::read_to_string(&config_file)
            .with_context(|| format!("Failed to read config from {:?}", &config_file))?;

        Ok(config_file_content)
    }

    /// Retrieve the host's distribution.
    ///
    /// Checks `/etc/os-release` for the "NAME" field and retrieves the value. Returns "unknown" if
    /// not successful.
    fn get_distro() -> Result<String> {
        let os_release_file = std::fs::File::open("/etc/os-release");
        match os_release_file {
            Ok(file) => {
                let reader = std::io::BufReader::new(file);
                for line in reader.lines() {
                    if let Ok(line) = line {
                        if line.starts_with("ID=") {
                            return Ok(line.trim_start_matches("ID=").trim().to_string());
                        }
                    }
                }
                Ok("unknown".to_string())
            }
            Err(e) => {
                error!("Could not open '/etc/os-release, defaulting to 'unknown'\n{e:?}");
                Ok("unknown".to_string())
            }
        }
    }

    /// Retrieve the hostname.
    ///
    /// Returns "unknown" if not successful.
    fn get_hostname() -> Result<String> {
        match nix::unistd::gethostname() {
            Ok(hostname) => {
                match hostname.into_string() {
                    Ok(host) => Ok(host),
                    Err(e) => {
                        error!("Could not determine hostname, defaulting to 'unknown'.\n {:?}", e);
                        Ok("unknown".to_string())
                    }
                }
            },
            Err(e) => {
                error!("Could not determine hostname, defaulting to 'unknown'.\n {:?}", e);
                Ok("unknown".to_string())
            }
        }
    }

    /// Initialize the [ConfigFile] struct.
    ///
    /// If found, it parses the config file and tries to expand all paths. If the config file is
    /// absent or fields are missing it will use default values (see [ConfigFile]).
    pub(crate) fn init() -> Result<ConfigFile> {
        let conf_string = match Self::read_config_file() {
            Ok(s) => s,
            Err(e) => {
                // If no config file could be be found, return an empty string which will be parsed
                // by toml and does nothing.
                warn!("{:?}", e);
                warn!("Default config values will be used");
                "".to_string()
            }
        };

        // Intermediate struct for the parsed config file data
        #[derive(Deserialize)]
        struct ParsedFile {
            config_root: Option<String>,
            modules_root: Option<String>,
            hosts_root: Option<String>,
            hostname: Option<String>,
            distribution: Option<String>,
            use_sudo: Option<bool>,
            deploy_sys_files: Option<bool>,
            intall_pkg_cmd: Option<VecDeque<String>>,
            remove_pkg_cmd: Option<VecDeque<String>>,
            skip_pkg_install: Option<bool>,
        }

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

        // Construct the config
        Ok(ConfigFile {
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
            deploy_sys_files: parsed_data.deploy_sys_files.unwrap_or(true),
            intall_pkg_cmd: parsed_data.intall_pkg_cmd,
            skip_pkg_install: parsed_data.skip_pkg_install.unwrap_or(false),
            remove_pkg_cmd: parsed_data.remove_pkg_cmd
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    #[derive(Serialize)]
    struct TestConf {
        config_root: Option<String>,
        modules_root: Option<String>,
        hosts_root: Option<String>,
    }

    #[test]
    fn test_create_config() -> Result<()> {
        let test_config = TestConf {
            config_root: Some("/tmp".to_string()),
            modules_root: None,
            hosts_root: None,
        };

        std::env::set_var("XDG_CONFIG_HOME", "/tmp");
        assert_eq!(env::var("XDG_CONFIG_HOME"), Ok("/tmp".to_string()));

        // No config file present
        let conf = ConfigFile::init()?;
        assert_eq!(
            conf.config_root,
            PathBuf::from(shellexpand::full("~/.dotfiles").unwrap().to_string(),)
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

        // With config file
        std::fs::create_dir_all("/tmp/dotdeploy/")?;
        let _ = std::fs::write("/tmp/dotdeploy/config.toml", toml::to_string(&test_config)?);

        let conf = ConfigFile::init()?;

        assert_eq!(conf.config_root, PathBuf::from("/tmp"));
        assert_eq!(conf.modules_root, PathBuf::from("/tmp/modules"));
        assert_eq!(conf.hosts_root, PathBuf::from("/tmp/hosts"));

        let test_config2 = TestConf {
            config_root: Some("/foo".to_string()),
            modules_root: Some("/bar".to_string()),
            hosts_root: Some("/baz".to_string()),
        };
        std::fs::remove_dir_all("/tmp/dotdeploy/")?;

        std::fs::create_dir_all("/tmp/dotdeploy/")?;
        let _ = std::fs::write(
            "/tmp/dotdeploy/config.toml",
            toml::to_string(&test_config2)?,
        );

        let conf = ConfigFile::init()?;
        assert_eq!(conf.config_root, PathBuf::from("/foo"));
        assert_eq!(conf.modules_root, PathBuf::from("/bar"));
        assert_eq!(conf.hosts_root, PathBuf::from("/baz"));
        std::fs::remove_dir_all("/tmp/dotdeploy/")?;

        Ok(())
    }
}
