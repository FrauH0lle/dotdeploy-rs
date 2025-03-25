//! This module manages the tracking of the deployment process.
//!
//! Provided functionality include the creation of checksums for files, backing them up, storing
//! their source and target as well as their associated module.

use crate::config::DotdeployConfig;
use crate::modules::messages::CommandMessage;
use crate::phases::DeployPhaseStruct;
use crate::store::sqlite_checksums::{StoreSourceFileChecksum, StoreTargetFileChecksum};
use crate::store::sqlite_files::StoreFile;
use crate::store::sqlite_modules::StoreModule;
use crate::utils::FileUtils;
use crate::utils::file_fs;
use crate::utils::sudo::PrivilegeManager;
use color_eyre::eyre::{OptionExt, WrapErr, eyre};
use color_eyre::{Result, Section};
use sqlite::SQLiteStore;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::debug;

pub(crate) mod sqlite;
pub(crate) mod sqlite_backups;
pub(crate) mod sqlite_checksums;
pub(crate) mod sqlite_files;
pub(crate) mod sqlite_modules;

// -------------------------------------------------------------------------------------------------
// Stores
// -------------------------------------------------------------------------------------------------

#[derive(Debug)]
pub(crate) struct Stores {
    pub(crate) user_store: SQLiteStore,
    pub(crate) system_store: Option<SQLiteStore>,
}

impl Stores {
    /// Creates a new instance of the [`Stores`] databases.
    ///
    /// Creates and initializes both user and system stores based on configuration. The system store
    /// is only created if [`field@DotdeployConfig::deploy_sys_files`] is enabled in the config.
    ///
    /// # Arguments
    /// * `config` - The [`DotdeployConfig`] containing store paths and settings
    ///
    /// # Errors
    /// Returns an error if initialization of either store fails.
    pub(crate) async fn new(config: &DotdeployConfig, pm: Arc<PrivilegeManager>) -> Result<Self> {
        Ok(Self {
            user_store: sqlite::init_sqlite_store(config, false, Arc::clone(&pm))
                .await
                .wrap_err("Failed to initialize user store")?,
            system_store: if config.deploy_sys_files {
                Some(
                    sqlite::init_sqlite_store(config, true, Arc::clone(&pm))
                        .await
                        .wrap_err("Failed to initialize system store")?,
                )
            } else {
                None
            },
        })
    }

    /// Determines the appropriate store (user or system) for a given file path.
    ///
    /// Selects the store based on the file's location:
    /// - User store: Files within the user's home directory
    /// - System store: Files outside home directory (requires system store configuration)
    ///
    /// # Arguments
    /// * `file_path` - Path to the file being operated on. Implements `AsRef<Path>`
    ///                 for flexible input types.
    ///
    /// # Errors
    /// Returns an error if:
    /// - Home directory cannot be determined (user store check)
    /// - System store is requested but not configured (check `deploy_sys_files` config)
    fn match_store<P: AsRef<Path>>(&self, file_path: P) -> Result<&SQLiteStore> {
        // Convert input to Path reference for comparison
        let path = file_path.as_ref();
        
        // Check if path is within user's home directory
        let home_dir = dirs::home_dir()
            .ok_or_eyre("Failed to determine user's home directory")
            .suggestion("Verify $HOME environment variable is set")?;

        if path.starts_with(&home_dir) {
            // User store for home directory files
            Ok(&self.user_store)
        } else {
            // System store for paths outside home directory
            self.system_store.as_ref().ok_or_eyre(
                "Attempted system store access but system deployments are disabled"
            ).suggestion("Enable 'deploy_sys_files' in configuration to use system store")
        }
    }

    // --
    // * Modules

    pub(crate) async fn add_module(&self, module: &StoreModule) -> Result<()> {
        self.user_store.add_module(module).await?;
        if let Some(ref system_store) = self.system_store {
            system_store.add_module(module).await?;
        }
        Ok(())
    }

    pub(crate) async fn remove_module<S: AsRef<str>>(&self, module: S) -> Result<()> {
        self.user_store.remove_module(&module).await?;
        if let Some(ref system_store) = self.system_store {
            system_store.remove_module(&module).await?;
        }
        Ok(())
    }

    pub(crate) async fn get_all_modules(&self) -> Result<Vec<StoreModule>> {
        let mut seen: HashSet<String> = HashSet::new();
        let modules = self
            .user_store
            .get_all_modules()
            .await?
            .into_iter()
            .chain(if let Some(system_store) = &self.system_store {
                system_store.get_all_modules().await?.into_iter()
            } else {
                vec![].into_iter()
            })
            .filter(|m| {
                if seen.contains(&m.name) {
                    false
                } else {
                    seen.insert(m.name.clone());
                    true
                }
            })
            .collect::<Vec<_>>();

        Ok(modules)
    }

    // --
    // * File operations

    pub(crate) async fn get_file<P: AsRef<Path>>(&self, filename: P) -> Result<Option<StoreFile>> {
        let store = self.match_store(&filename)?;
        store.get_file(&filename).await
    }

    pub(crate) async fn add_file(&self, file: StoreFile) -> Result<()> {
        let store = self.match_store(&file.target)?;
        store.add_file(file).await
    }

    pub(crate) async fn remove_file<S: AsRef<str>>(&self, file: S) -> Result<()> {
        let store = self.match_store(file.as_ref())?;
        store.remove_file(&file).await
    }

    pub(crate) async fn get_all_files<S: AsRef<str>>(&self, module: S) -> Result<Vec<StoreFile>> {
        let mut files = self.user_store.get_all_files(&module).await?;
        if let Some(ref system_store) = self.system_store {
            files.append(&mut system_store.get_all_files(&module).await?)
        }
        Ok(files)
    }

    pub(crate) async fn check_file_exists<P: AsRef<Path>>(&self, path: P) -> Result<bool> {
        let store = self.match_store(&path)?;
        store.check_file_exists(&path).await
    }

    // --
    // * File checksums

    pub(super) async fn get_source_checksum<P: AsRef<Path>>(
        &self,
        filename: P,
    ) -> Result<StoreSourceFileChecksum> {
        let store = self.match_store(&filename)?;
        store.get_source_checksum(&filename).await
    }

    pub(super) async fn get_target_checksum<P: AsRef<Path>>(
        &self,
        filename: P,
    ) -> Result<StoreTargetFileChecksum> {
        let store = self.match_store(&filename)?;
        store.get_target_checksum(&filename).await
    }

    pub(super) async fn get_all_source_checksums(&self) -> Result<Vec<StoreSourceFileChecksum>> {
        let mut checksums = self.user_store.get_all_source_checksums().await?;
        if let Some(ref system_store) = self.system_store {
            checksums.append(&mut system_store.get_all_source_checksums().await?)
        }
        Ok(checksums)
    }

    pub(super) async fn get_all_target_checksums(&self) -> Result<Vec<StoreTargetFileChecksum>> {
        let mut checksums = self.user_store.get_all_target_checksums().await?;
        if let Some(ref system_store) = self.system_store {
            checksums.append(&mut system_store.get_all_target_checksums().await?)
        }
        Ok(checksums)
    }

    // --
    // * Backups

    pub(crate) async fn add_backup<P: AsRef<Path>>(&self, file_path: P) -> Result<()> {
        let store = self.match_store(&file_path)?;
        store.add_backup(&file_path).await
    }

    pub(crate) async fn add_dummy_backup<P: AsRef<Path>>(&self, file_path: P) -> Result<()> {
        let store = self.match_store(&file_path)?;
        store.add_dummy_backup(&file_path).await
    }

    pub(crate) async fn remove_backup<P: AsRef<Path>>(&self, file_path: P) -> Result<()> {
        let store = self.match_store(&file_path)?;
        store.remove_backup(&file_path).await
    }

    pub(crate) async fn check_backup_exists<P: AsRef<Path>>(&self, path: P) -> Result<bool> {
        let store = self.match_store(&path)?;
        store.check_backup_exists(&path).await
    }

    pub(crate) async fn restore_backup<P: AsRef<Path>>(&self, file_path: P, to: P) -> Result<()> {
        let store = self.match_store(&file_path)?;
        store.restore_backup(&file_path, &to).await
    }

    // --
    // * Packages

    pub(crate) async fn add_package<S: AsRef<str>>(&self, module: S, package: S) -> Result<()> {
        self.user_store.add_package(&module, &package).await?;
        if let Some(ref system_store) = self.system_store {
            system_store.add_package(&module, &package).await?
        }

        Ok(())
    }

    pub(crate) async fn remove_package<S: AsRef<str>>(&self, module: S, package: S) -> Result<()> {
        self.user_store.remove_package(&module, &package).await?;
        if let Some(ref system_store) = self.system_store {
            system_store.remove_package(&module, &package).await?
        }

        Ok(())
    }

    pub(crate) async fn get_all_module_packages<S: AsRef<str>>(
        &self,
        module: S,
    ) -> Result<Vec<String>> {
        let usr_pkgs = self.user_store.get_all_module_packages(&module).await?;
        let sys_pkgs = if let Some(ref system_store) = self.system_store {
            system_store.get_all_module_packages(&module).await?
        } else {
            vec![]
        };

        let unique: HashSet<String> =
            HashSet::from_iter(usr_pkgs.into_iter().chain(sys_pkgs.into_iter()));

        Ok(unique.into_iter().collect::<Vec<_>>())
    }

    pub(crate) async fn get_all_other_module_packages<S: AsRef<str>>(
        &self,
        module: S,
    ) -> Result<Vec<String>> {
        let usr_pkgs = self
            .user_store
            .get_all_other_module_packages(&module)
            .await?;
        let sys_pkgs = if let Some(ref system_store) = self.system_store {
            system_store.get_all_other_module_packages(&module).await?
        } else {
            vec![]
        };

        let unique: HashSet<String> =
            HashSet::from_iter(usr_pkgs.into_iter().chain(sys_pkgs.into_iter()));

        Ok(unique.into_iter().collect::<Vec<_>>())
    }
}

// -------------------------------------------------------------------------------------------------
// Store trait
// -------------------------------------------------------------------------------------------------

pub(crate) trait Store {
    /// Returns true if this is system-wide store.
    ///
    /// A store can be a system-wide store (true) or user-specific store (false).
    fn is_system(&self) -> bool;

    /// Returns the path of store location.
    fn path(&self) -> &PathBuf;

    // --
    // * Modules

    /// Adds or updates a module in the database.
    ///
    /// If a module with the same name already exists, this operation will be ignored.
    ///
    /// # Arguments
    /// * `module` - The `StoreModule` to be added or updated.
    ///
    /// # Errors
    /// Returns an error if there's an error during the database operation.
    async fn add_module(&self, module: &StoreModule) -> Result<()>;

    /// Removes a module from the database.
    ///
    /// # Arguments
    /// * `module` - The name of the module to be removed.
    ///
    /// # Errors
    /// Returns an error if there's an error during the database operation.
    async fn remove_module<S: AsRef<str>>(&self, module: S) -> Result<()>;

    /// Retrieves a single module from the store by its name.
    ///
    /// If no module is found, `None` is returned.
    /// 
    /// # Arguments
    /// * `name` - The name of the module to retrieve.
    ///
    /// # Errors
    /// Returns an error if there's an error during the database operation or if the module is not
    /// found.
    async fn get_module<S: AsRef<str>>(&self, name: S) -> Result<Option<StoreModule>>;

    /// Retrieves all modules from the store.
    async fn get_all_modules(&self) -> Result<Vec<StoreModule>>;

    // --
    // * File operations

    /// Retrieves a single file entry from the store based on its filename.
    ///
    /// # Arguments
    /// * `filename` - The path of the file to retrieve.
    ///
    /// # Errors
    /// Returns an error if there's an error during the database operation or if the file is not
    /// found.
    async fn get_file<P: AsRef<Path>>(&self, filename: P) -> Result<Option<StoreFile>>;

    /// Adds or updates a single file entry in the database.
    ///
    /// # Arguments
    /// * `file` - The `StoreFile` to be added or updated.
    ///
    /// # Errors
    /// Returns an error if there's an error during the database operation.
    async fn add_file(&self, file: StoreFile) -> Result<()>;

    /// Removes a single file entry from the database.
    ///
    /// # Arguments
    /// * `file` - The destination path of the file to be removed.
    ///
    /// # Errors
    /// Returns an error if there's an error during the database operation.
    async fn remove_file<S: AsRef<str>>(&self, file: S) -> Result<()>;

    /// Retrieves all file entries associated with a specific module.
    ///
    /// # Arguments
    /// * `module` - The name of the module to retrieve files for.
    ///
    /// # Errors
    /// Returns an error if there's an error during the database operation.
    async fn get_all_files<S: AsRef<str>>(&self, module: S) -> Result<Vec<StoreFile>>;

    /// Checks if a file exists in the store database.
    ///
    /// # Arguments
    /// * `path` - The path of the file to check.
    ///
    /// # Errors
    /// Returns an error if there's an error during the database operation.
    async fn check_file_exists<P: AsRef<Path>>(&self, path: P) -> Result<bool>;

    // --
    // * File checksums

    /// Retrieves the checksum of a source file from the store database if it exists.
    ///
    /// # Arguments
    ///
    /// * `filename` - The path of the destination file to retrieve the checksum for.
    ///
    /// # Errors
    /// Returns an error if there's an error during the database operation.
    async fn get_source_checksum<P: AsRef<Path>>(
        &self,
        filename: P,
    ) -> Result<StoreSourceFileChecksum>;

    /// Retrieves the checksum of a destination file from the store database if it exists..
    ///
    /// # Arguments
    ///
    /// * `filename` - The path of the destination file to retrieve the checksum for.
    ///
    /// # Errors
    /// Returns an error if there's an error during the database operation.
    async fn get_target_checksum<P: AsRef<Path>>(
        &self,
        filename: P,
    ) -> Result<StoreTargetFileChecksum>;

    /// Retrieves all source checksums from the store database.
    ///
    /// # Errors
    /// Returns an error if there's an error during the database operation.
    async fn get_all_source_checksums(&self) -> Result<Vec<StoreSourceFileChecksum>>;

    /// Retrieves all destination checksums from the store database.
    ///
    /// # Errors
    /// Returns an error if there's an error during the database operation.
    async fn get_all_target_checksums(&self) -> Result<Vec<StoreTargetFileChecksum>>;

    // --
    // * Backups

    /// Adds a backup of a file to the store database.
    ///
    /// This method handles both regular files and symlinks, collecting necessary metadata and file
    /// content before storing it in the database.
    ///
    /// # Arguments
    /// * `file_path` - The path of the file to backup.
    async fn add_backup<P: AsRef<Path>>(&self, file_path: P) -> Result<()>;

    async fn add_dummy_backup<P: AsRef<Path>>(&self, file_path: P) -> Result<()>;

    /// Removes a backup entry from the store database.
    ///
    /// # Arguments
    /// * `file_path` - The path of the file whose backup should be removed.
    ///
    /// # Errors
    /// Returns an error if:
    /// - Database connection fails
    /// - Delete operation fails
    async fn remove_backup<P: AsRef<Path>>(&self, file_path: P) -> Result<()>;

    /// Checks if a backup of a file exists in the store database.
    ///
    /// # Arguments
    /// * `path` - The path of the file to check for a backup
    ///
    /// # Errors
    /// Returns an error if:
    /// - Path contains invalid Unicode characters
    /// - Database connection fails
    /// - Query execution fails
    async fn check_backup_exists<P: AsRef<Path>>(&self, path: P) -> Result<bool>;

    /// Restores a backup from the store database to a specified location.
    ///
    /// Fetches the backup entry and restores it based on its type (symlink or regular file).
    /// Preserves original file metadata including permissions and ownership.
    ///
    /// # Arguments
    /// * `file_path` - The original path of the backed-up file
    /// * `to` - The path where the backup should be restored
    ///
    /// # Errors
    /// Returns an error if:
    /// - Backup entry cannot be found in database
    /// - File paths contain invalid Unicode
    /// - File creation or permission changes fail
    /// - Elevated permissions are needed but sudo fails
    async fn restore_backup<P: AsRef<Path>>(&self, file_path: P, to: P) -> Result<()>;

    // --
    // * Packages

    /// Add a package as installed to the store database.
    async fn add_package<S: AsRef<str>>(&self, module: S, package: S) -> Result<()>;

    /// Remove a package as installed to the store database.
    async fn remove_package<S: AsRef<str>>(&self, module: S, package: S) -> Result<()>;

    /// Get all packages installed by a module
    async fn get_all_module_packages<S: AsRef<str>>(&self, module: S) -> Result<Vec<String>>;

    /// Get all packages installed by all other modules
    async fn get_all_other_module_packages<S: AsRef<str>>(&self, module: S) -> Result<Vec<String>>;

    /// Get all packages installed
    async fn get_all_packages(&self) -> Result<Vec<String>>;

    // --
    // * Removal & Updates

    // FIXME 2025-03-22: All of this is pretty redundant and can probably be implemented similar to
    //   https://users.rust-lang.org/t/limit-a-generic-type-to-be-either-a-or-b/66367/7
    async fn cache_command<S: AsRef<str>>(&self, phase: S, data: DeployPhaseStruct) -> Result<()>;

    async fn get_cached_commands<S: AsRef<str>>(&self, phase: S) -> Result<Option<DeployPhaseStruct>>;

    async fn cache_message<S: AsRef<str>>(&self, command: S, message: CommandMessage)
    -> Result<()>;

    async fn get_all_cached_messages<S: AsRef<str>>(
        &self,
        module: S,
        command: S,
    ) -> Result<Vec<CommandMessage>>;

    async fn remove_all_cached_messages<S: AsRef<str>>(&self, module: S, command: S) -> Result<()>;
}

// -------------------------------------------------------------------------------------------------
// Store directioy creation
// -------------------------------------------------------------------------------------------------

/// Creates the directory for a system-wide store with elevated permissions.
///
/// This function creates the directory structure for storing system-wide data, using sudo to set
/// appropriate permissions (777) that allow all users to write.
///
/// # Arguments
/// * `file_path` - Path where the system store directory should be created
///
/// # Returns
/// * `Ok(())` - Directory created successfully or already exists
/// * `Err` - If directory creation or permission setting fails
async fn create_system_dir<P: AsRef<Path>>(file_path: P, pm: Arc<PrivilegeManager>) -> Result<()> {
    let file_utils = FileUtils::new(Arc::clone(&pm));
    match file_utils.check_file_exists(file_path.as_ref()).await {
        Ok(false) => {
            debug!(
                "Store directory '{}' does not exist, creating",
                file_path.as_ref().display()
            );

            // Create the directory with sudo
            file_utils
                .ensure_dir_exists(&file_path)
                .await
                .wrap_err_with(|| format!("Failed to create directory {:?}", file_path.as_ref()))?;

            // Set permissions to allow all users to write to the directory
            pm.sudo_exec(
                "chmod",
                ["777", file_fs::path_to_string(&file_path)?.as_str()],
                Some("Adjusting permissions of system store DB directory"),
            )
            .await
            .wrap_err_with(|| {
                format!(
                    "Failed to change permissions of directory {:?}",
                    file_path.as_ref()
                )
            })?;

            Ok(())
        }
        Ok(true) => {
            debug!(
                "Store directory '{}' exists already, continuing.",
                &file_path.as_ref().display()
            );
            Ok(())
        }
        Err(e) => Err(eyre!("{}", e)),
    }
}

/// Creates the directory for a user-specific store.
///
/// This function creates the directory structure for storing user-specific data with standard user
/// permissions.
///
/// # Arguments
/// * `file_path` - Path where the user store directory should be created
///
/// # Returns
/// * `Ok(())` - Directory created successfully or already exists
/// * `Err` - If directory creation fails
async fn create_user_dir<P: AsRef<Path>>(file_path: P, pm: Arc<PrivilegeManager>) -> Result<()> {
    let file_utils = FileUtils::new(pm);
    match file_path.as_ref().try_exists() {
        Ok(false) => {
            debug!(
                "Store directory '{}' does not exist, creating.",
                file_path.as_ref().display()
            );
            file_utils
                .ensure_dir_exists(file_path.as_ref())
                .await
                .wrap_err_with(|| format!("Failed to create directory {:?}", file_path.as_ref()))?;
            Ok(())
        }
        Ok(true) => {
            debug!(
                "Store directory '{}' exists already, continuing.",
                file_path.as_ref().display()
            );
            Ok(())
        }
        Err(e) => Err(eyre!("{}", e)),
    }
}

// -------------------------------------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::pm_setup;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_create_user_dir() -> Result<()> {
        let (_tx, pm) = tests::pm_setup()?;

        let temp_dir = TempDir::new()?;
        let test_path = temp_dir.path().join("user_store");

        // --
        // * Test creating new directory
        create_user_dir(&test_path, Arc::clone(&pm)).await?;
        assert!(test_path.exists(), "create user store directory");

        // --
        // * Test idempotency - should not fail when directory exists
        create_user_dir(&test_path, Arc::clone(&pm)).await?;

        Ok(())
    }

    // Note: create_system_dir tests are limited since they require sudo
    // Real tests would need to run with elevated privileges
    #[tokio::test]
    async fn test_create_system_dir() -> Result<()> {
        let (_tx, pm) = tests::pm_setup()?;
        let file_utils = FileUtils::new(Arc::clone(&pm));

        let temp_dir = TempDir::new()?;
        let test_path = temp_dir.path().join("system_store");

        pm.sudo_exec(
            "chown",
            ["root:root", temp_dir.path().to_str().unwrap()],
            None,
        )
        .await?;
        pm.sudo_exec("chmod", ["600", temp_dir.path().to_str().unwrap()], None)
            .await?;

        // --
        // * Test creating new directory
        create_system_dir(&test_path, Arc::clone(&pm)).await?;
        assert!(
            file_utils.check_file_exists(&test_path).await?,
            "create system store directory"
        );

        // --
        // * Test idempotency - should not fail when directory exists
        create_system_dir(&test_path, Arc::clone(&pm)).await?;

        Ok(())
    }
}
