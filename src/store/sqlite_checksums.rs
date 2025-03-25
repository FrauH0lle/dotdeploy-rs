//! This module handles checksum operations for files in the dotdeploy store.
//!
//! It provides functionality to retrieve and manage checksums for both source and destination
//! files, which is crucial for tracking file changes and ensuring data integrity during the dotfile
//! deployment process.

use std::path::{Path, PathBuf};

/// Represents a source file checksum record from the store database
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct StoreSourceFileChecksum {
    /// Path to the file
    pub(crate) source: Option<PathBuf>,
    /// Checksum of the file contents
    pub(crate) source_checksum: Option<String>,
}

/// Represents a destination file checksum record from the store database
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct StoreTargetFileChecksum {
    /// Path to the file
    pub(crate) target: PathBuf,
    /// Checksum of the file contents
    pub(crate) target_checksum: Option<String>,
}

impl StoreSourceFileChecksum {
    pub(crate) fn new<P: AsRef<Path>>(source: Option<P>, source_checksum: Option<String>) -> Self {
        StoreSourceFileChecksum {
            source: source.map(|s| s.as_ref().to_path_buf()),
            source_checksum,
        }
    }
}

impl StoreTargetFileChecksum {
    pub(crate) fn new<P: AsRef<Path>>(target: P, target_checksum: Option<String>) -> Self {
        StoreTargetFileChecksum {
            target: target.as_ref().to_path_buf(),
            target_checksum,
        }
    }
}

//
// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use crate::store::sqlite::tests::store_setup_helper;
    use color_eyre::Result;

    #[tokio::test]
    async fn test_get_checksums() -> Result<()> {
        let store = store_setup_helper("link").await?;

        // Get single checksum
        let result = store.get_source_checksum("/home/foo2.txt").await?;
        assert_eq!(
            result,
            StoreSourceFileChecksum::new(
                Some("/dotfiles/foo2.txt".to_string()),
                Some("source_checksum2".to_string())
            )
        );
        let result = store.get_target_checksum("/home/foo3.txt").await?;
        assert_eq!(
            result,
            StoreTargetFileChecksum::new(
                "/home/foo3.txt".to_string(),
                Some("dest_checksum3".to_string())
            )
        );
        let result = store.get_target_checksum("/does/not/exist.txt").await?;
        assert_eq!(
            result,
            StoreTargetFileChecksum::new("/does/not/exist.txt".to_string(), None)
        );

        // Source file and source checksum missing
        let store = store_setup_helper("create").await?;
        let result = store.get_source_checksum("/home/foo2.txt").await?;
        assert_eq!(result, StoreSourceFileChecksum::new::<PathBuf>(None, None));

        // All checksums
        let store = store_setup_helper("create").await?;
        let result = store.get_all_source_checksums().await?;
        assert_eq!(result.len(), 0);

        let store = store_setup_helper("create").await?;
        let result = store.get_all_target_checksums().await?;
        assert_eq!(result.len(), 5);

        Ok(())
    }
}
