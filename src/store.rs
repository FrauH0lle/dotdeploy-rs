//! This module manages the tracking of the deployment process.
//!
//! Provided functionality include the creation of checksums for files, backing them up, storing
//! their source and target as well as their associated module.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use sqlite::SQLiteStore;

// use self::init::{init_system_store, init_user_store};
use crate::config::DotdeployConfig;
use crate::utils::file_fs;
use crate::utils::sudo;

pub(crate) mod sqlite;
// pub(crate) mod backups;
// pub(crate) mod checksums;
// pub(crate) mod db;
// pub(crate) mod errors;
// pub(crate) mod files;
pub(crate) mod init;
// pub(crate) mod modules;

// #[cfg(test)]
// pub(crate) mod tests;

pub(crate) struct Stores {
    pub(crate) user_store: SQLiteStore,
    pub(crate) system_store: Option<SQLiteStore>,
}

// pub(crate) struct Stores<T: Store> {
//     pub(crate) user_store: T,
//     pub(crate) system_store: Option<T>,
// }

// impl<T> Stores<T>
// where
//     T: Store,
// {
//     pub(crate) async fn init(config: &DotdeployConfig) -> Result<Self> {
//         Ok(Self {
//             user_store: init::init_user_store(None)
//                 .await
//                 .context("Failed to initialize user store")?,
//             system_store: None, // system_store: if config.deploy_sys_files {
//                                 //     Some(
//                                 //         init_system_store()
//                                 //             .await
//                                 //             .context("Failed to initialize system store")?,
//                                 //     )
//                                 // } else {
//                                 //     None
//                                 // },
//         })
//     }
// }

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
