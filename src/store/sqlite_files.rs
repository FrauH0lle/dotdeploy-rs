//! This module provides functionality for managing file entries in the dotdeploy store database.
//!
//! It includes operations for adding, removing, retrieving, and checking the existence of file
//! records.

use derive_builder::Builder;

/// Representation of a store file entry (row) in the database.
#[derive(Clone, Debug, PartialEq, Eq, Default, Builder)]
#[builder(setter(prefix = "with"))]
pub(crate) struct StoreFile {
    /// The module associated with this file
    #[builder(setter(into))]
    pub(crate) module: String,
    /// The source path of the file (optional)
    pub(crate) source: Option<String>,
    /// The source path of the file (optional, byte vector)
    pub(crate) source_u8: Option<Vec<u8>>,
    /// The checksum of the source file (optional)
    pub(crate) source_checksum: Option<String>,
    /// The destination path of the file
    #[builder(setter(into))]
    pub(crate) target: String,
    /// The destination path of the file (byte vector)
    #[builder(setter(into))]
    pub(crate) target_u8: Vec<u8>,
    /// The checksum of the destination file (optional)
    pub(crate) target_checksum: Option<String>,
    /// The operation performed on the file (must be either 'link', 'copy', or 'create')
    #[builder(setter(into))]
    pub(crate) operation: String,
    /// The user associated with this file operation (optional)
    pub(crate) user: Option<String>,
    /// The date and time when the file entry was added or last updated
    pub(crate) date: chrono::DateTime<chrono::Utc>,
}

// -------------------------------------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DotdeployConfig;
    use crate::store::Store;
    use crate::store::sqlite::init_sqlite_store;
    use crate::store::sqlite::tests::store_setup_helper;
    use crate::store::sqlite_modules::StoreModuleBuilder;
    use crate::tests;
    use crate::utils::common::os_str_to_bytes;
    use color_eyre::Result;
    use std::{ffi::OsString, str::FromStr};
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_add_and_get_file() -> Result<()> {
        let temp_dir = tempdir()?;
        let (_tx, pm) = tests::pm_setup()?;

        // Init store
        let config = DotdeployConfig {
            user_store_path: temp_dir.path().to_path_buf(),
            ..Default::default()
        };
        let user_store = init_sqlite_store(&config, pm).await?;

        // Insert a module
        let test_module = StoreModuleBuilder::default()
            .with_name("test")
            .with_location("/testpath")
            .with_location_u8(os_str_to_bytes(OsString::from_str("/testpath")?))
            .with_user(Some("user".to_string()))
            .with_reason("manual")
            .with_depends(None)
            .with_date(chrono::offset::Utc::now())
            .build()?;

        user_store.add_module(&test_module).await?;

        // Test input
        let local_time = chrono::offset::Utc::now();
        let test_file = StoreFileBuilder::default()
            .with_module("test")
            .with_source(Some("/dotfiles/foo.txt".to_string()))
            .with_source_u8(Some(os_str_to_bytes(OsString::from_str(
                "/dotfiles/foo.txt",
            )?)))
            .with_source_checksum(Some("abc123".to_string()))
            .with_target("/home/foo.txt")
            .with_target_u8(os_str_to_bytes(OsString::from_str("/home/foo.txt")?))
            .with_target_checksum(Some("abc123".to_string()))
            .with_operation("link")
            .with_user(Some(whoami::username()))
            .with_date(local_time)
            .build()?;

        user_store.add_file(test_file.clone()).await?;

        let result = user_store.get_file("/home/foo.txt").await?;

        assert_eq!(Some(test_file), result);

        // Missing file
        let e = user_store.get_file("/doesNotExist.txt").await?;
        assert!(e.is_none());

        Ok(())
    }

    #[tokio::test]
    async fn test_get_all_files() -> Result<()> {
        let store = store_setup_helper("link").await?;
        let result = store.get_all_files("test").await?;

        assert_eq!(result.len(), 5);
        assert_eq!(result[2].module, "test");
        assert_eq!(result[2].target, "/home/foo2.txt");

        // Missing module
        let e = store.get_all_files("Foobar").await?;
        assert!(e.is_empty());

        Ok(())
    }
}
