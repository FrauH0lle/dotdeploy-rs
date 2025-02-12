//! This module handles checksum operations for files in the dotdeploy store.
//!
//! It provides functionality to retrieve and manage checksums for both source and destination
//! files, which is crucial for tracking file changes and ensuring data integrity during the dotfile
//! deployment process.

/// Represents a source file checksum record from the store database
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct StoreSourceFileChecksum {
    /// Path to the file
    pub(crate) source: Option<String>,
    /// Checksum of the file contents
    pub(crate) source_checksum: Option<String>,
}

/// Represents a destination file checksum record from the store database
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct StoreDestFileChecksum {
    /// Path to the file
    pub(crate) destination: String,
    /// Checksum of the file contents
    pub(crate) destination_checksum: Option<String>,
}

impl StoreSourceFileChecksum {
    pub(crate) fn new(source: Option<String>, source_checksum: Option<String>) -> Self {
        StoreSourceFileChecksum {
            source,
            source_checksum,
        }
    }
}

impl StoreDestFileChecksum {
    pub(crate) fn new(destination: String, destination_checksum: Option<String>) -> Self {
        StoreDestFileChecksum {
            destination,
            destination_checksum,
        }
    }
}

//
// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::sqlite::tests::store_setup_helper;
    use crate::store::Store;
    use color_eyre::Result;

    #[tokio::test]
    async fn test_get_checksums() -> Result<()> {
        let store = store_setup_helper("link").await?;

        // Get single checksum
        let result = store.get_source_checksum("/home/foo2.txt").await?;
        assert_eq!(
            result,
            Some(StoreSourceFileChecksum::new(
                Some("/dotfiles/foo2.txt".to_string()),
                Some("source_checksum2".to_string())
            ))
        );
        let result = store.get_destination_checksum("/home/foo3.txt").await?;
        assert_eq!(
            result,
            Some(StoreDestFileChecksum::new(
                "/home/foo3.txt".to_string(),
                Some("dest_checksum3".to_string())
            ))
        );
        let result = store
            .get_destination_checksum("/does/not/exist.txt")
            .await?;
        assert_eq!(result, None);

        // Source file and source checksum missing
        let store = store_setup_helper("create").await?;
        let result = store.get_source_checksum("/home/foo2.txt").await?;
        assert_eq!(result, Some(StoreSourceFileChecksum::new(None, None)));

        // All checksums
        let store = store_setup_helper("create").await?;
        let result = store.get_all_src_checksums().await?;
        assert_eq!(result.len(), 0);

        let store = store_setup_helper("create").await?;
        let result = store.get_all_dest_checksums().await?;
        assert_eq!(result.len(), 5);

        Ok(())
    }
}
