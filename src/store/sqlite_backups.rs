//! This module provides functionality for managing file backups in the dotdeploy store database.
//!
//! It includes operations for adding, removing, checking, and restoring backups.

use crate::store::sqlite::SQLiteStore;
use crate::utils::FileUtils;
use crate::utils::common::{bytes_to_os_str, os_str_to_bytes};
use crate::utils::file_metadata;
use color_eyre::Result;
use color_eyre::eyre::{WrapErr, eyre};
use derive_builder::Builder;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use tokio::fs;
use tokio::io::{AsyncWriteExt, BufWriter};

/// Representation of a store backup entry (row) in the database.
#[derive(Clone, Debug, PartialEq, Eq, Default, Builder)]
#[builder(setter(prefix = "with"))]
pub(crate) struct StoreBackup {
    /// Absolute file path of the backed-up file (human-readable)
    #[builder(setter(into))]
    pub(crate) path: String,
    /// Absolute file path of the backed-up file (byte vector)
    pub(crate) path_u8: Vec<u8>,
    /// Type of the file: "link" or "regular"
    #[builder(setter(into))]
    pub(crate) file_type: String,
    /// Binary content of the file (for regular files)
    pub(crate) content: Option<Vec<u8>>,
    /// Absolute file path to the source (for symlinks, human-readable)
    pub(crate) link_source: Option<String>,
    /// Absolute file path to the source (for symlinks, byte vector)
    pub(crate) link_source_u8: Option<Vec<u8>>,
    /// User and group as string (UID:GID)
    #[builder(setter(into))]
    pub(crate) owner: String,
    /// File permissions
    pub(crate) permissions: Option<u32>,
    /// SHA256 checksum of the file
    pub(crate) checksum: Option<String>,
    /// Date and time when the backup was created
    pub(crate) date: chrono::DateTime<chrono::Utc>,
}

impl SQLiteStore {
    /// Creates a backup entry for a symbolic link in the store database.
    ///
    /// Creates a new [`StoreBackup`] instance configured for a symbolic link, preserving the link
    /// target path and ownership information.
    ///
    /// # Arguments
    /// * `file_path_str` - String representation of the symlink's path
    /// * `metadata` - File metadata containing symlink information and permissions
    ///
    /// # Errors
    /// Returns an error if:
    /// - Symlink target path cannot be converted to a string
    /// - User/group IDs cannot be extracted from metadata
    pub(crate) fn create_symlink_backup<P: AsRef<Path>>(
        &self,
        file_path: P,
        metadata: &file_metadata::FileMetadata,
    ) -> Result<StoreBackup> {
        let (user_id, group_id) = self
            .get_user_and_group_id(metadata, &file_path)
            .wrap_err_with(|| {
                format!(
                    "Could not get UID and GUI of {:?}",
                    file_path.as_ref().display()
                )
            })?;

        Ok(StoreBackupBuilder::default()
            .with_path(file_path.as_ref().to_string_lossy())
            .with_path_u8(os_str_to_bytes(file_path.as_ref()))
            .with_file_type("link")
            .with_content(None)
            .with_link_source(
                metadata
                    .symlink_source
                    .as_ref()
                    .map(|s| s.to_string_lossy().to_string()),
            )
            .with_link_source_u8(metadata.symlink_source.as_ref().map(os_str_to_bytes))
            .with_owner(format!("{}:{}", user_id, group_id))
            .with_permissions(None)
            .with_checksum(None)
            .with_date(chrono::offset::Utc::now())
            .build()?)
    }

    /// Creates a backup entry for a regular file in the store database.
    ///
    /// Creates a new [`StoreBackup`] instance configured for a regular file, preserving the file
    /// target path, ownership and permission information.
    /// # Arguments
    /// * `file_path` - Path to the file to backup
    /// * `file_path_str` - String representation of the file path
    /// * `metadata` - File metadata containing permissions and checksums
    ///
    /// # Errors
    /// Returns an error if:
    /// - File target path cannot be converted to a string
    /// - User/group IDs cannot be extracted from metadata
    /// - Permissions cannot be extracted from metadata
    /// - File checksum cannot be calculated
    pub(crate) async fn create_regular_file_backup<P: AsRef<Path>>(
        &self,
        file_path: P,
        metadata: file_metadata::FileMetadata,
    ) -> Result<StoreBackup> {
        let (user_id, group_id) = self
            .get_user_and_group_id(&metadata, &file_path)
            .wrap_err_with(|| {
                format!(
                    "Could not get UID and GUI of {:?}",
                    file_path.as_ref().display()
                )
            })?;
        let permissions = metadata.permissions.ok_or_else(|| {
            eyre!(
                "Could not get permissions of {:?}",
                file_path.as_ref().display()
            )
        })?;
        let checksum = metadata.checksum.ok_or_else(|| {
            eyre!(
                "Could not get checksum of {:?}",
                file_path.as_ref().display()
            )
        })?;

        let content = self.read_file_content(&file_path).await?;

        Ok(StoreBackupBuilder::default()
            .with_path(file_path.as_ref().to_string_lossy())
            .with_path_u8(os_str_to_bytes(file_path.as_ref()))
            .with_file_type("regular")
            .with_content(Some(content))
            .with_link_source(None)
            .with_link_source_u8(None)
            .with_owner(format!("{}:{}", user_id, group_id))
            .with_permissions(Some(permissions))
            .with_checksum(Some(checksum))
            .with_date(chrono::offset::Utc::now())
            .build()?)
    }

    /// Extracts user and group IDs from file metadata.
    ///
    /// # Arguments
    /// * `metadata` - File metadata containing ownership information
    /// * `file_path_str` - String representation of file path, used for error messages
    ///
    /// # Errors
    /// Returns an error if:
    /// - UID is missing from metadata
    /// - GID is missing from metadata
    fn get_user_and_group_id<P: AsRef<Path>>(
        &self,
        metadata: &file_metadata::FileMetadata,
        file_path: P,
    ) -> Result<(u32, u32)> {
        let user_id = metadata
            .uid
            .ok_or_else(|| eyre!("Could not get UID of {}", file_path.as_ref().display()))?;
        let group_id = metadata
            .gid
            .ok_or_else(|| eyre!("Could not get GID of {}", file_path.as_ref().display()))?;
        Ok((user_id, group_id))
    }

    /// Reads the content of a file, with fallback to elevated permissions if needed.
    ///
    /// First attempts to read the file normally, then falls back to elevated permissions if access
    /// is denied.
    ///
    /// # Arguments
    /// * `file_path` - Path to the file to read
    ///
    /// # Errors
    /// Returns an error if:
    /// - File does not exist
    /// - File cannot be read even with elevated permissions
    async fn read_file_content<P: AsRef<Path>>(&self, file_path: P) -> Result<Vec<u8>> {
        match fs::read(&file_path).await {
            Ok(c) => Ok(c),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                self.read_file_with_elevated_permissions(file_path).await
            }
            Err(e) => {
                Err(e).wrap_err_with(|| format!("Failed to read {:?}", file_path.as_ref()))?
            }
        }
    }

    /// Reads a file using elevated permissions via sudo.
    ///
    /// Creates a temporary copy of the file with sudo, makes it readable, then reads its contents.
    ///
    /// # Arguments
    /// * `file_path` - Path to the file that needs elevated permissions to read
    ///
    /// # Errors
    /// Returns an error if:
    /// - Temporary file creation fails
    /// - Sudo copy operation fails
    /// - File permission changes fail
    /// - Reading the temporary copy fails
    async fn read_file_with_elevated_permissions<P: AsRef<Path>>(
        &self,
        file_path: P,
    ) -> Result<Vec<u8>> {
        let temp_file = tempfile::NamedTempFile::new()?;

        self.privilege_manager
            .sudo_exec(
                OsString::from_str("cp")?,
                [
                    OsString::from_str("--preserve")?,
                    OsString::from_str("--no-dereference")?,
                    OsString::from(file_path.as_ref()),
                    OsString::from(temp_file.path()),
                ],
                Some(
                    format!(
                        "Create temporary copy of {} for backup creation",
                        file_path.as_ref().display()
                    )
                    .as_str(),
                ),
            )
            .await?;

        let file_utils = FileUtils::new(Arc::clone(&self.privilege_manager));
        file_utils
            .set_file_metadata(
                &temp_file,
                file_metadata::FileMetadata {
                    uid: None,
                    gid: None,
                    permissions: Some(0o777),
                    is_symlink: false,
                    symlink_source: None,
                    checksum: None,
                },
            )
            .await?;

        fs::read(&temp_file)
            .await
            .wrap_err_with(|| format!("Failed to read {}", &temp_file.path().display()))
    }

    /// Inserts a backup entry into the store database.
    ///
    /// Stores the backup information including file content, metadata, and timestamps. Uses ON
    /// CONFLICT DO NOTHING to handle duplicate paths.
    ///
    /// # Arguments
    /// * `b_file` - The [`StoreBackup`] entry to insert
    ///
    /// # Errors
    /// Returns an error if:
    /// - Database connection fails
    /// - Insert operation fails
    pub(crate) async fn insert_backup_into_db(&self, b_file: StoreBackup) -> Result<()> {
        sqlx::query!(
            r#"
INSERT INTO backups (path, path_u8, file_type, content, link_source, link_source_u8, owner, permissions, checksum, date)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
ON CONFLICT(path) DO NOTHING
            "#,
            b_file.path,
            b_file.path_u8,
            b_file.file_type,
            b_file.content,
            b_file.link_source,
            b_file.link_source_u8,
            b_file.owner,
            b_file.permissions,
            b_file.checksum,
            b_file.date
        )
        .execute(&self.pool)
        .await
        .wrap_err_with(|| format!("Failed to insert backup of {}", b_file.path))?;

        Ok(())
    }

    /// Fetches a backup entry from the store database.
    ///
    /// Retrieves all backup information including file content, metadata, and timestamps for a
    /// specific file path.
    ///
    /// # Arguments
    /// * `file_path_str` - String representation of the file path to fetch
    ///
    /// # Errors
    /// Returns an error if:
    /// - No backup exists for the given path
    /// - Database query fails
    pub(crate) async fn fetch_backup_from_db<P: AsRef<Path>>(
        &self,
        file_path: P,
    ) -> Result<StoreBackup> {
        let file_path_u8 = os_str_to_bytes(file_path.as_ref());

        let backup = sqlx::query_as!(
            StoreBackup,
            r#"
SELECT path, path_u8, file_type, content, link_source, link_source_u8, owner, permissions as "permissions: u32", checksum, date as "date: chrono::DateTime<chrono::Utc>"
FROM backups where path_u8 = ?1
            "#,
            file_path_u8
        ).fetch_one(&self.pool).await.wrap_err_with(|| format!("Failed to fetch backup for {} from store", file_path.as_ref().display()))?;

        Ok(backup)
    }

    /// Restores a symbolic link backup to the filesystem.
    ///
    /// Creates a new symbolic link pointing to the original target path. Falls back to using sudo
    /// if normal creation fails due to permissions.
    ///
    /// # Arguments
    /// * `backup` - The [`StoreBackup`] entry containing link information
    /// * `to` - Path where the symbolic link should be created
    ///
    /// # Errors
    /// Returns an error if:
    /// - Link creation fails even with elevated permissions
    /// - Setting ownership or permissions fails
    pub(crate) async fn restore_symlink_backup<P: AsRef<Path>>(
        &self,
        backup: &StoreBackup,
        to: P,
    ) -> Result<()> {
        let link_source_os = bytes_to_os_str(
            backup
                .link_source_u8
                .as_ref()
                .ok_or_else(|| eyre!("Link source for {} not found", &backup.path))?,
        );
        match fs::symlink(&link_source_os, &to).await {
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                self.privilege_manager
                    .sudo_exec(
                        OsString::from_str("ln")?,
                        [
                            OsString::from_str("-sf")?,
                            link_source_os,
                            OsString::from(to.as_ref()),
                        ],
                        None,
                    )
                    .await?;
            }
            Err(e) => {
                return Err(e)
                    .wrap_err_with(|| format!("Failed to restore backup of {:?}", &backup.path))?;
            }
        }

        let owner: Vec<&str> = backup.owner.split(':').collect();
        let permissions: Option<u32> = backup.permissions;

        self.set_file_metadata(to.as_ref(), owner, permissions, true)
            .await?;
        Ok(())
    }

    /// Restores a regular file backup to the filesystem.
    ///
    /// Writes file content and restores original metadata. Uses a temporary location if the target
    /// requires elevated permissions.
    ///
    /// # Arguments
    /// * `backup` - The [`StoreBackup`] entry containing file data
    /// * `to` - Path where the file should be restored
    ///
    /// # Errors
    /// Returns an error if:
    /// - File creation fails
    /// - Writing content fails
    /// - Setting metadata fails
    /// - Moving file to final location fails
    pub(crate) async fn restore_regular_file_backup<P: AsRef<Path>>(
        &self,
        backup: &StoreBackup,
        to: P,
    ) -> Result<()> {
        let (write_dest, file) = self.prepare_write_destination(&to).await?;

        self.write_backup_content(file, &backup.content.clone().unwrap(), &write_dest)
            .await?;

        let owner: Vec<&str> = backup.owner.split(':').collect();
        let permissions: Option<u32> = backup.permissions;

        self.set_file_metadata(&write_dest, owner, permissions, false)
            .await?;

        if write_dest != to.as_ref() {
            self.move_file_with_sudo(&write_dest, to.as_ref()).await?;
            // Ensure temporary file is removed
            fs::remove_file(&write_dest).await.ok();
        }

        Ok(())
    }

    /// Prepares the write destination for restoring a backup file.
    ///
    /// Attempts to create the file at the target location, falling back to a temporary location if
    /// permissions are insufficient.
    ///
    /// # Arguments
    /// * `to` - Intended destination path for the restored file
    ///
    /// # Errors
    /// Returns an error if:
    /// - File creation fails at both target and temporary locations
    /// - Temporary file creation fails
    async fn prepare_write_destination<P: AsRef<Path>>(
        &self,
        to: P,
    ) -> Result<(PathBuf, fs::File)> {
        match fs::File::create(&to).await {
            Ok(f) => Ok((to.as_ref().to_path_buf(), f)),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                let temp_file = tempfile::NamedTempFile::new()?;
                // As soon as temp_file goes out of scope, its desctructor runs and deletes the
                // file. However, we need to keep the file alive and make sure it gets removed
                // manually.
                let (_file, temp_path) = temp_file.keep()?;
                let file = fs::File::create(&temp_path).await?;
                Ok((temp_path, file))
            }
            Err(e) => {
                Err(e).wrap_err_with(|| format!("Failed to create file at {:?}", to.as_ref()))?
            }
        }
    }

    /// Writes backup content to the specified file handle.
    ///
    /// Uses buffered writing for efficiency when restoring file content.
    ///
    /// # Arguments
    /// * `file` - Open file handle to write to
    /// * `content` - Byte content to write
    /// * `write_dest` - Path to file (for error messages)
    ///
    /// # Errors
    /// Returns an error if:
    /// - Writing content fails
    /// - Flushing buffers fails
    async fn write_backup_content(
        &self,
        file: fs::File,
        content: &[u8],
        write_dest: &Path,
    ) -> Result<()> {
        let mut writer = BufWriter::new(file);
        writer
            .write(content)
            .await
            .wrap_err_with(|| format!("Failed to write to file {:?}", write_dest))?;
        writer
            .flush()
            .await
            .wrap_err("Failed to flush write buffer")?;
        Ok(())
    }

    /// Sets file metadata (owner and permissions) for a restored file.
    ///
    /// Applies original ownership and permission settings to the restored file. Handles both
    /// regular files and symbolic links.
    ///
    /// # Arguments
    /// * `path` - Path to the restored file
    /// * `owner` - Vector containing user and group IDs
    /// * `permissions` - Optional file mode to set
    /// * `is_symlink` - Whether the file is a symbolic link
    ///
    /// # Errors
    /// Returns an error if:
    /// - Owner IDs cannot be parsed
    /// - Setting metadata fails
    async fn set_file_metadata(
        &self,
        path: &Path,
        owner: Vec<&str>,
        permissions: Option<u32>,
        is_symlink: bool,
    ) -> Result<()> {
        let file_utils = FileUtils::new(Arc::clone(&self.privilege_manager));

        file_utils
            .set_file_metadata(
                path,
                file_metadata::FileMetadata {
                    uid: Some(owner[0].parse::<u32>()?),
                    gid: Some(owner[1].parse::<u32>()?),
                    permissions,
                    is_symlink,
                    symlink_source: None,
                    checksum: None,
                },
            )
            .await?;
        Ok(())
    }

    /// Moves a file using sudo when elevated permissions are required.
    ///
    /// Copies the file preserving all attributes, using sudo to handle permission restrictions at
    /// the destination.
    ///
    /// # Arguments
    /// * `from` - Source file path
    /// * `to` - Destination path requiring elevated permissions
    ///
    /// # Errors
    /// Returns an error if:
    /// - Sudo command fails
    /// - File paths contain invalid characters
    async fn move_file_with_sudo(&self, from: &Path, to: &Path) -> Result<()> {
        self.privilege_manager
            .sudo_exec(
                "cp",
                ["--preserve", from.to_str().unwrap(), to.to_str().unwrap()],
                None,
            )
            .await?;
        Ok(())
    }
}

// -------------------------------------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use crate::store::sqlite::tests::store_setup_helper;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_file_backup() -> Result<()> {
        let store = store_setup_helper("link").await?;

        let temp_path = tempdir()?;

        fs::write(temp_path.path().join("foo.txt"), b"Hello World!").await?;
        fs::set_permissions(
            temp_path.path().join("foo.txt"),
            std::fs::Permissions::from_mode(0o666),
        )
        .await?;

        // Backup file
        store.add_backup(&temp_path.path().join("foo.txt")).await?;
        fs::remove_file(temp_path.path().join("foo.txt")).await?;
        assert!(!temp_path.path().join("foo.txt").exists());

        // Restore file
        store
            .restore_backup(
                &temp_path.path().join("foo.txt"),
                &temp_path.path().join("foo.txt"),
            )
            .await?;
        assert!(temp_path.path().join("foo.txt").exists());

        let meta = temp_path.path().join("foo.txt").metadata()?;
        let mode = meta.mode();
        let user = meta.uid();
        let group = meta.gid();

        assert_eq!(format!("{:o}", mode), format!("{:o}", 33206));

        assert_eq!(user, nix::unistd::getuid().as_raw());
        assert_eq!(group, nix::unistd::getgid().as_raw());

        Ok(())
    }
}
