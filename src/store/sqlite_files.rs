//! This module provides functionality for managing file entries in the dotdeploy store database.
//!
//! It includes operations for adding, removing, retrieving, and checking the existence of file
//! records.

/// Representation of a store file entry (row) in the database.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
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
    pub(crate) date: chrono::DateTime<chrono::Utc>,
}

impl StoreFile {
    pub(crate) fn new(
        module: String,
        source: Option<String>,
        source_checksum: Option<String>,
        destination: String,
        destination_checksum: Option<String>,
        operation: String,
        user: Option<String>,
        date: chrono::DateTime<chrono::Utc>,
    ) -> Self {
        StoreFile {
            module,
            source,
            source_checksum,
            destination,
            destination_checksum,
            operation,
            user,
            date,
        }
    }
}

// -------------------------------------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DotdeployConfig;
    use crate::store::sqlite::init_sqlite_store;
    use crate::store::sqlite::tests::store_setup_helper;
    use crate::store::sqlite_modules::StoreModule;
    use crate::store::Store;
    use color_eyre::Result;
    use std::env;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_add_and_get_file() -> Result<()> {
        let temp_dir = tempdir()?;
        // Init store
        let config = DotdeployConfig {
            user_store_path: temp_dir.path().to_path_buf(),
            ..Default::default()
        };
        let user_store = init_sqlite_store(&config, false).await?;

        // Insert a module
        let test_module = StoreModule {
            name: "test".to_string(),
            location: "/testpath".to_string(),
            user: Some("user".to_string()),
            reason: "manual".to_string(),
            depends: None,
            date: chrono::offset::Utc::now(),
        };

        user_store.add_module(test_module).await?;

        // Test input
        let local_time = chrono::offset::Utc::now();
        let test_file = StoreFile {
            module: "test".to_string(),
            source: Some("/dotfiles/foo.txt".to_string()),
            source_checksum: Some("abc123".to_string()),
            destination: "/home/foo.txt".to_string(),
            destination_checksum: Some("abc123".to_string()),
            operation: "link".to_string(),
            user: Some(env::var("USER")?),
            date: local_time,
        };

        user_store.add_file(test_file.clone()).await?;

        let result = user_store.get_file("/home/foo.txt").await?;

        assert_eq!(test_file, result);

        // Missing file
        let e = user_store.get_file("/doesNotExist.txt").await;
        assert!(e.is_err());

        Ok(())
    }

    #[tokio::test]
    async fn test_get_all_files() -> Result<()> {
        let store = store_setup_helper("link").await?;
        let result = store.get_all_files("test").await?;

        assert_eq!(result.len(), 5);
        assert_eq!(result[2].module, "test");
        assert_eq!(result[2].destination, "/home/foo2.txt");

        // Missing module
        let e = store.get_all_files("Foobar").await?;
        assert!(e.is_empty());

        Ok(())
    }
}
