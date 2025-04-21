//! This module manages the tracking of the deployment process.
//!
//! Provided functionality include the creation of checksums for files, backing them up, storing
//! their source and target as well as their associated module.

use crate::modules::messages::CommandMessage;
use crate::phases::task::PhaseTask;
use crate::store::sqlite_checksums::{StoreSourceFileChecksum, StoreTargetFileChecksum};
use crate::store::sqlite_files::StoreFile;
use crate::store::sqlite_modules::StoreModule;
use crate::utils::FileUtils;
use crate::utils::sudo::PrivilegeManager;
use color_eyre::Result;
use color_eyre::eyre::{WrapErr, eyre};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::debug;
use uuid::Uuid;

pub(crate) mod sqlite;
pub(crate) mod sqlite_backups;
pub(crate) mod sqlite_checksums;
pub(crate) mod sqlite_files;
pub(crate) mod sqlite_modules;

// -------------------------------------------------------------------------------------------------
// Store trait
// -------------------------------------------------------------------------------------------------

pub(crate) trait Store {
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
    async fn remove_file<P: AsRef<Path>>(&self, file: P) -> Result<()>;

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

    // --
    // * Tasks

    async fn get_task_uuids<S: AsRef<str>>(&self, module: S) -> Result<Vec<Uuid>>;

    async fn add_task(&self, data: PhaseTask) -> Result<()>;

    async fn get_tasks<S: AsRef<str>>(&self, module: S) -> Result<Vec<PhaseTask>>;

    async fn get_task(&self, uuid: Uuid) -> Result<Option<PhaseTask>>;

    async fn remove_task(&self, uuid: Uuid) -> Result<()>;

    // --
    // * Messages

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
}
