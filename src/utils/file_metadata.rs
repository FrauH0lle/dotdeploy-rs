//! File metadata handling module.
//!
//! This module provides functionality to get and set file metadata, including permissions,
//! ownership, and checksums. It handles privilege elevation when necessary, allowing operations on
//! files that might require higher permissions.

use crate::utils::FileUtils;
use crate::utils::file_fs;
use crate::utils::file_permissions;
use color_eyre::{Result, eyre::WrapErr};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use tokio::fs;

/// Represents the metadata of a file or symbolic link.
///
/// This struct is used to encapsulate various metadata attributes of a file or symbolic link,
/// including ownership, permissions, and checksum information.
pub(crate) struct FileMetadata {
    /// User ID of the file owner
    pub(crate) uid: Option<u32>,
    /// Group ID of the file
    pub(crate) gid: Option<u32>,
    /// File permissions in octal format
    pub(crate) permissions: Option<u32>,
    /// Whether the file is a symbolic link
    pub(crate) is_symlink: bool,
    /// The target of the symbolic link, if applicable
    pub(crate) symlink_source: Option<PathBuf>,
    /// SHA256 checksum of the file contents
    pub(crate) checksum: Option<String>,
}

impl FileUtils {
    /// Retrieves file metadata, elevating privileges if necessary.
    ///
    /// This function attempts to get the metadata of a file or symbolic link. If permission is
    /// denied, it creates a temporary copy of the file with elevated privileges to retrieve the
    /// metadata.
    ///
    /// # Arguments
    ///
    /// * `path` - The path to the file or symbolic link.
    ///
    /// # Errors
    ///
    /// Returns an error if retrieval fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::path::Path;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     let metadata = get_file_metadata(Path::new("/path/to/file")).await?;
    ///     println!("File permissions: {:o}", metadata.permissions.unwrap_or(0));
    ///     Ok(())
    /// }
    /// ```
    pub(crate) async fn get_file_metadata<P: AsRef<Path>>(&self, path: P) -> Result<FileMetadata> {
        let metadata = match fs::symlink_metadata(&path).await {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                // If permission is denied, create a temporary copy with elevated privileges
                let temp_file = tempfile::NamedTempFile::new()?;
                let temp_path_str = file_fs::path_to_string(&temp_file)?;

                self.privilege_manager
                    .sudo_exec(
                        "cp",
                        [
                            "--preserve",
                            "--no-dereference",
                            &file_fs::path_to_string(&path)?,
                            &temp_path_str,
                        ],
                        Some(
                            format!(
                                "Create temporary copy of {:?} for metadata retrieval",
                                &path.as_ref()
                            )
                            .as_str(),
                        ),
                    )
                    .await?;

                fs::symlink_metadata(&temp_file)
                    .await
                    .wrap_err_with(|| format!("Failed to get metadata of {:?}", &temp_file))?
            }
            Err(e) => Err(e)
                .wrap_err_with(|| format!("Falied to get metadata of {:?}", &path.as_ref()))?,
        };

        Ok(FileMetadata {
            uid: Some(metadata.uid()),
            gid: Some(metadata.gid()),
            permissions: Some(metadata.mode()),
            is_symlink: metadata.is_symlink(),
            symlink_source: if metadata.is_symlink() {
                Some(fs::read_link(&path).await?)
            } else {
                None
            },
            checksum: if metadata.is_symlink() {
                None
            } else {
                Some(self.calculate_sha256_checksum(&path).await?)
            },
        })
    }

    /// Sets file metadata, elevating privileges if necessary.
    ///
    /// This function attempts to set the metadata (permissions and ownership) of a file or symbolic
    /// link. If permission is denied, it uses sudo to perform the operations.
    ///
    /// # Arguments
    ///
    /// * `path` - The path to the file or symbolic link.
    /// * `metadata` - The `FileMetadata` struct containing the metadata to set.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::path::Path;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     let metadata = FileMetadata {
    ///         uid: Some(1000),
    ///         gid: Some(1000),
    ///         permissions: Some(0o644),
    ///         is_symlink: false,
    ///         symlink_source: None,
    ///         checksum: None,
    ///     };
    ///     set_file_metadata(Path::new("/path/to/file"), metadata).await?;
    ///     Ok(())
    /// }
    /// ```
    pub(crate) async fn set_file_metadata<P: AsRef<Path>>(
        &self,
        path: P,
        metadata: FileMetadata,
    ) -> Result<()> {
        // Set file permissions if specified
        if let Some(permissions) = metadata.permissions {
            match fs::set_permissions(&path, std::fs::Permissions::from_mode(permissions)).await {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    // Use sudo to set permissions if permission is denied
                    self.privilege_manager
                        .sudo_exec(
                            "chmod",
                            [
                                file_permissions::perms_int_to_str(permissions)?.as_str(),
                                file_fs::path_to_string(&path)?.as_str(),
                            ],
                            None,
                        )
                        .await?
                }
                Err(e) => Err(e).wrap_err_with(|| {
                    format!("Failed to set permissions for {:?}", &path.as_ref())
                })?,
            }
        }
        // Set file ownership if specified
        if let (Some(uid), Some(gid)) = (metadata.uid, metadata.gid) {
            match std::os::unix::fs::lchown(&path, Some(uid), Some(gid)) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    // Use sudo to set ownership if permission is denied
                    self.privilege_manager
                        .sudo_exec(
                            "chown",
                            [
                                format!("{}:{}", uid, gid).as_str(),
                                &file_fs::path_to_string(&path)?,
                            ],
                            None,
                        )
                        .await?
                }
                Err(e) => Err(e).wrap_err_with(|| {
                    format!("Failed to set user and group for {:?}", &path.as_ref())
                })?,
            }
        }

        Ok(())
    }
}

//
// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_get_file_metadata() -> Result<()> {
        let (_tx, pm) = tests::pm_setup()?;
        let fs_utils = FileUtils::new(Arc::clone(&pm));

        let temp_file = tempfile::NamedTempFile::new()?;
        let meta = fs_utils.get_file_metadata(temp_file.path()).await?;
        assert_eq!(meta.uid, Some(nix::unistd::getuid().as_raw()));
        assert_eq!(meta.gid, Some(nix::unistd::getgid().as_raw()));
        assert_eq!(
            file_permissions::perms_int_to_str(meta.permissions.unwrap())?,
            "600"
        );
        assert!(!meta.is_symlink);

        // Symlink
        let temp_dir = tempfile::tempdir()?;
        let temp_link = temp_dir.path().join("foo.txt");
        fs::symlink(temp_file.path(), &temp_link).await?;
        let meta = fs_utils.get_file_metadata(&temp_link).await?;
        assert_eq!(
            file_permissions::perms_int_to_str(meta.permissions.unwrap())?,
            "777"
        );
        assert!(meta.is_symlink);
        assert_eq!(meta.symlink_source, Some(temp_file.path().to_path_buf()));

        // Test with elevated permissions
        let temp_file = tempfile::NamedTempFile::new()?;
        pm.sudo_exec(
            "chown",
            ["root:root", temp_file.path().to_str().unwrap()],
            None,
        )
        .await?;
        pm.sudo_exec("chmod", ["644", temp_file.path().to_str().unwrap()], None)
            .await?;

        let meta = fs_utils.get_file_metadata(temp_file.path()).await?;
        assert_eq!(meta.uid, Some(0));
        assert_eq!(meta.gid, Some(0));
        assert_eq!(
            file_permissions::perms_int_to_str(meta.permissions.unwrap())?,
            "644"
        );
        assert!(!meta.is_symlink);

        // Symlink
        let temp_dir = tempfile::tempdir()?;
        let temp_link = temp_dir.path().join("foo.txt");
        fs::symlink(temp_file.path(), &temp_link).await?;
        pm.sudo_exec("chown", ["root:root", temp_link.to_str().unwrap()], None)
            .await?;
        let meta = fs_utils.get_file_metadata(&temp_link).await?;
        assert_eq!(
            file_permissions::perms_int_to_str(meta.permissions.unwrap())?,
            "777"
        );
        assert!(meta.is_symlink);
        assert_eq!(meta.symlink_source, Some(temp_file.path().to_path_buf()));

        Ok(())
    }

    #[tokio::test]
    async fn test_set_file_metadata() -> Result<()> {
        let (_tx, pm) = tests::pm_setup()?;
        let fs_utils = FileUtils::new(Arc::clone(&pm));

        let temp_file = tempfile::NamedTempFile::new()?;
        fs_utils
            .set_file_metadata(
                temp_file.path(),
                FileMetadata {
                    uid: None,
                    gid: None,
                    permissions: Some(0o777),
                    is_symlink: false,
                    symlink_source: None,
                    checksum: None,
                },
            )
            .await?;
        let meta = fs_utils.get_file_metadata(temp_file.path()).await?;
        assert_eq!(
            file_permissions::perms_int_to_str(meta.permissions.unwrap())?,
            "777"
        );

        fs_utils
            .set_file_metadata(
                temp_file.path(),
                FileMetadata {
                    uid: Some(0),
                    gid: Some(0),
                    permissions: Some(0o644),
                    is_symlink: false,
                    symlink_source: None,
                    checksum: None,
                },
            )
            .await?;
        let meta = fs_utils.get_file_metadata(temp_file.path()).await?;
        assert_eq!(meta.uid, Some(0));
        assert_eq!(meta.gid, Some(0));
        assert_eq!(
            file_permissions::perms_int_to_str(meta.permissions.unwrap())?,
            "644"
        );

        Ok(())
    }
}
