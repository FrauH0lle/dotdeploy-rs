//! This module provides functions for initializing user and system stores for the dotdeploy
//! application.

use std::env;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::PathBuf;

use anyhow::{Context, Result};
use tokio::fs;

use crate::store::db::Store;
use crate::store::errors::SQLiteError;

/// Initialize the user store.
///
/// This function creates and initializes a SQLite database for storing user-specific dotdeploy
/// data.
///
/// The database location is determined based on the following priority:
/// 1. The provided `path` argument
/// 2. `$XDG_DATA_HOME/dotdeploy`
/// 3. `$HOME/.local/share/dotdeploy`
///
/// # Arguments
/// * `path` - An optional custom path for the user store.
///
/// # Returns
/// * `Ok(Store)` if the store is successfully initialized.
/// * `Err(SQLiteError)` if an error occurs during initialization.
pub(crate) async fn init_user_store(path: Option<PathBuf>) -> Result<Store, SQLiteError> {
    // Determine the store path based on the provided path or environment variables
    let store_path: PathBuf = path.unwrap_or_else(|| {
        if let Ok(xdg_dir) = env::var("XDG_DATA_HOME") {
            // Use XDG_DATA_HOME if available
            [xdg_dir.as_str(), "dotdeploy"].iter().collect()
        } else {
            // Fallback to HOME/.local/share/dotdeploy
            [
                env::var("HOME")
                    .map_err(|e| SQLiteError::Other(e.into()))
                    .expect("HOME environment variable not set")
                    .as_str(),
                ".local",
                "share",
                "dotdeploy",
            ]
            .iter()
            .collect()
        }
    });

    // Create a new Store instance and initialize it
    let mut store = Store::new(store_path.clone(), false);
    store
        .init()
        .await
        .map_err(|e| e.into_anyhow())
        .with_context(|| {
            format!(
                "Failed to initialize user store in {}",
                &store_path.display()
            )
        })?;
    Ok(store)
}

/// Initialize the system store.
///
/// This function creates and initializes a SQLite database for storing system-wide dotdeploy data.
/// The database is always created at `/var/lib/dotdeploy`.
///
/// # Returns
/// * `Ok(Store)` if the store is successfully initialized.
/// * `Err(SQLiteError)` if an error occurs during initialization.
pub(crate) async fn init_system_store() -> Result<Store, SQLiteError> {
    // Set the fixed path for the system store
    let store_path: PathBuf = PathBuf::from("/var/lib/dotdeploy");

    // Create a new Store instance and initialize it
    let mut store = Store::new(store_path.clone(), true);
    store
        .init()
        .await
        .map_err(|e| e.into_anyhow())
        .context("Failed to initialize system store in /var/lib/dotdeploy")?;

    // Set permissions for the store file to be readable and writable by all users
    fs::set_permissions(&store.path, std::fs::Permissions::from_mode(0o666))
        .await
        .map_err(|e| SQLiteError::Other(e.into()))?;
    Ok(store)
}

//
// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    use crate::store::db;
    use crate::store::modules::StoreModule;

    #[tokio::test]
    async fn test_init_user_store() -> Result<(), SQLiteError> {
        let temp_dir = tempdir().map_err(|e| SQLiteError::Other(e.into()))?;

        // Init store
        let user_store = init_user_store(Some(temp_dir.into_path())).await?;

        // Insert a module
        let test_module = StoreModule {
            name: "test".to_string(),
            location: "/testpath".to_string(),
            user: Some("user".to_string()),
            reason: "manual".to_string(),
            depends: None,
            date: chrono::offset::Local::now(),
        };

        user_store.add_module(test_module).await?;

        let conn = user_store.get_con().await?;
        let result = conn
            .interact(move |conn| -> Result<String, SQLiteError> {
                db::prepare_connection(conn)?;
                Ok(conn.query_row("SELECT name FROM modules WHERE id=1", [], |row| row.get(0))?)
            })
            .await??;

        assert_eq!(result, "test".to_string());
        Ok(())
    }
}
