//! This module manages the tracking of the deployment process.
//!
//! Provided functionality include the creation of checksums for files, backing them up, storing
//! their source and target as well as their associated module.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{bail, Context, Result};

use self::db::SQLiteStore;
use self::init::{init_system_store, init_user_store};
use crate::utils::file_fs;
use crate::utils::sudo;
use crate::DEPLOY_SYSTEM_FILES;

pub(crate) mod backups;
pub(crate) mod checksums;
pub(crate) mod db;
pub(crate) mod errors;
pub(crate) mod files;
pub(crate) mod init;
pub(crate) mod modules;

#[cfg(test)]
pub(crate) mod tests;

pub(crate) struct Stores<T: Store> {
    pub(crate) user_store: T,
    pub(crate) system_store: Option<T>,
}

impl<T> Stores<T>
where
    T: Store,
{
    pub(crate) async fn init() -> Result<Self> {
        Ok(Self {
            user_store: init_user_store(None)
                .await
                .map_err(|e| e.into_anyhow())
                .context("Failed to initialize user store")?,
            system_store: if DEPLOY_SYSTEM_FILES.load(Ordering::Relaxed) {
                Some(
                    init_system_store()
                        .await
                        .map_err(|e| e.into_anyhow())
                        .context("Failed to initialize system store")?,
                )
            } else {
                None
            },
        })
    }
}

trait Store {
    /// Initializes a store database.
    ///
    /// This method creates the necessary directory and initializes the database.
    ///
    /// # Errors
    /// If the initialization is not successful, returns [`Err`].
    async fn init(&mut self) -> Result<()>;

    /// Returns true if this is system-wide store.
    ///
    /// A store can be a system-wide store (true) or user-specific store (false).
    fn is_system(&self) -> bool;

    /// Returns the path of store location.
    fn path(&self) -> &PathBuf;

    /// Creates the directory for the store if it doesn't exist.
    ///
    /// For system stores, this method uses sudo to create the directory and set appropriate
    /// permissions. For user stores, it creates the directory without elevated permissions.
    ///
    /// # Errors
    ///  Returns [`Err`] if an error occurs during directory creation.
    async fn create_dir(&self) -> Result<()> {
        if self.is_system() {
            create_system_dir(self.path()).await
        } else {
            create_user_dir(self.path()).await
        }
    }

    // File operations
    async fn get_file<P: AsRef<Path>>(&self, filename: P) -> Result<StoreFile>;
    async fn add_file(&self, file: StoreFile) -> Result<()>;
    async fn remove_file<S: AsRef<str>>(&self, file: S) -> Result<()>;
    async fn get_all_files<S: AsRef<str>>(&self, module: S) -> Result<Vec<StoreFile>>;
    async fn check_file_exists<P: AsRef<Path>>(&self, path: P) -> Result<bool>;
    // File checksums
    async fn get_source_checksum<P: AsRef<Path>>(
        &self,
        filename: P,
    ) -> Result<Option<(String, String)>>;
    async fn get_destination_checksum<P: AsRef<Path>>(
        &self,
        filename: P,
    ) -> Result<Option<(String, String)>>;
    async fn get_all_src_checksums(&self) -> Result<Vec<(Option<String>, Option<String>)>>;
    async fn get_all_dest_checksums(&self) -> Result<Vec<(Option<String>, Option<String>)>>;
    // Backups
    async fn add_backup<P: AsRef<Path>>(&self, file_path: P) -> Result<()>;
    async fn remove_backup<P: AsRef<Path>>(&self, file_path: P) -> Result<()>;
    async fn check_backup_exists<P: AsRef<Path>>(&self, path: P) -> Result<bool>;
    async fn restore_backup<P: AsRef<Path>>(&self, file_path: P, to: P) -> Result<()>;
}

/// Representation of a store file entry (row) in the database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StoreFile {
    /// The module associated with this file
    pub(crate) module: String,
    /// The source path of the file (optional)
    pub(crate) source: Option<String>,
    /// The checksum of the source file (optional)
    pub(crate) source_checksum: Option<String>,
    /// The destination path of the file
    pub(crate) destination: String,
    /// The checksum of the destination file (optional)
    pub(crate) destination_checksum: Option<String>,
    /// The operation performed on the file (must be either 'link', 'copy', or 'create')
    pub(crate) operation: String,
    /// The user associated with this file operation (optional)
    pub(crate) user: Option<String>,
    /// The date and time when the file entry was added or last updated
    pub(crate) date: chrono::DateTime<chrono::Local>,
}

/// Creates the directory for a system-wide store.
async fn create_system_dir<P: AsRef<Path>>(file_path: P) -> Result<()> {
    match file_path.as_ref().try_exists() {
        Ok(false) => {
            debug!(
                "Store directory '{}' does not exist, creating.",
                file_path.as_ref().display()
            );

            // Create the directory with sudo
            file_fs::ensure_dir_exists(&file_path)
                .await
                .with_context(|| format!("Failed to create directory {:?}", file_path.as_ref()))?;

            // Set permissions to allow all users to write to the directory
            sudo::sudo_exec(
                "chmod",
                &["777", file_fs::path_to_string(&file_path)?.as_str()],
                Some("Adjusting permissions of system store DB directory"),
            )
            .await
            .with_context(|| {
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
        Err(e) => bail!("{}", e),
    }
}

/// Creates the directory for a user-specific store.
async fn create_user_dir<P: AsRef<Path>>(file_path: P) -> Result<()> {
    match file_path.as_ref().try_exists() {
        Ok(false) => {
            debug!(
                "Store directory '{}' does not exist, creating.",
                file_path.as_ref().display()
            );
            file_fs::ensure_dir_exists(file_path.as_ref())
                .await
                .with_context(|| format!("Failed to create directory {:?}", file_path.as_ref()))?;
            Ok(())
        }
        Ok(true) => {
            debug!(
                "Store directory '{}' exists already, continuing.",
                file_path.as_ref().display()
            );
            Ok(())
        }
        Err(e) => bail!("{}", e),
    }
}
