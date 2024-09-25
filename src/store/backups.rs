//! This module provides functionality for managing file backups in the dotdeploy store database.
//!
//! It includes operations for adding, removing, checking, and restoring backups.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use deadpool_sqlite::rusqlite::params;
use tokio::fs;
use tokio::io::{AsyncWriteExt, BufWriter};

use crate::store::db;
use crate::store::errors::SQLiteError;
use crate::utils::file_fs;
use crate::utils::file_metadata;
use crate::utils::sudo;

/// Representation of a store backup entry (row) in the database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StoreBackup {
    /// Absolute file path of the backed-up file
    pub(crate) path: String,
    /// Type of the file: "link" or "regular"
    pub(crate) file_type: String,
    /// Binary content of the file (for regular files)
    pub(crate) content: Option<Vec<u8>>,
    /// Absolute file path to the source (for symlinks)
    pub(crate) link_source: Option<String>,
    /// User and group as string (UID:GID)
    pub(crate) owner: String,
    /// File permissions
    pub(crate) permissions: Option<u32>,
    /// SHA256 checksum of the file
    pub(crate) checksum: Option<String>,
    /// Date and time when the backup was created
    pub(crate) date: chrono::DateTime<chrono::Local>,
}

impl db::Store {
    /// Adds a backup of a file to the store database.
    ///
    /// This method handles both regular files and symlinks, collecting necessary metadata and file
    /// content before storing it in the database.
    ///
    /// # Arguments
    /// * `file_path` - The path of the file to backup.
    ///
    /// # Returns
    /// * `Ok(())` if the backup is successfully added.
    /// * `Err(SQLiteError)` if there's an error during the process.
    pub(crate) async fn add_backup<P: AsRef<Path>>(&self, file_path: P) -> Result<(), SQLiteError> {
        let file_path_str = file_fs::path_to_string(&file_path)?;
        let metadata = file_metadata::get_file_metadata(&file_path).await?;

        let b_file: StoreBackup = if metadata.is_symlink {
            self.create_symlink_backup(&file_path_str, &metadata)?
        } else {
            self.create_regular_file_backup(&file_path, &file_path_str, metadata)
                .await?
        };

        self.insert_backup_into_db(b_file).await
    }

    /// Creates a backup entry for a symlink.
    fn create_symlink_backup(
        &self,
        file_path_str: &str,
        metadata: &file_metadata::FileMetadata,
    ) -> Result<StoreBackup, SQLiteError> {
        let link_source = file_fs::path_to_string(metadata.symlink_source.clone().unwrap())?;
        let (user_id, group_id) = self.get_user_and_group_id(metadata, file_path_str)?;

        Ok(StoreBackup {
            path: file_path_str.to_string(),
            file_type: "link".to_string(),
            content: None,
            link_source: Some(link_source),
            owner: format!("{}:{}", user_id, group_id),
            permissions: None,
            checksum: None,
            date: chrono::offset::Local::now(),
        })
    }

    /// Creates a backup entry for a regular file.
    async fn create_regular_file_backup<P: AsRef<Path>>(
        &self,
        file_path: P,
        file_path_str: &str,
        metadata: file_metadata::FileMetadata,
    ) -> Result<StoreBackup, SQLiteError> {
        let (user_id, group_id) = self.get_user_and_group_id(&metadata, file_path_str)?;
        let permissions = metadata
            .permissions
            .ok_or_else(|| anyhow!("Could not get permissions of {:?}", file_path_str))?;
        let checksum = metadata
            .checksum
            .ok_or_else(|| anyhow!("Could not get checksum of {:?}", file_path_str))?;

        let content = self.read_file_content(&file_path).await?;

        Ok(StoreBackup {
            path: file_path_str.to_string(),
            file_type: "regular".to_string(),
            content: Some(content),
            link_source: None,
            owner: format!("{}:{}", user_id, group_id),
            permissions: Some(permissions),
            checksum: Some(checksum),
            date: chrono::offset::Local::now(),
        })
    }

    /// Retrieves user and group IDs from file metadata.
    fn get_user_and_group_id(
        &self,
        metadata: &file_metadata::FileMetadata,
        file_path_str: &str,
    ) -> Result<(u32, u32), SQLiteError> {
        let user_id = metadata
            .uid
            .ok_or_else(|| anyhow!("Could not get UID of {:?}", file_path_str))?;
        let group_id = metadata
            .gid
            .ok_or_else(|| anyhow!("Could not get GID of {:?}", file_path_str))?;
        Ok((user_id, group_id))
    }

    /// Reads the content of a file, handling permission issues if necessary.
    async fn read_file_content<P: AsRef<Path>>(
        &self,
        file_path: P,
    ) -> Result<Vec<u8>, SQLiteError> {
        match fs::read(&file_path).await {
            Ok(c) => Ok(c),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                self.read_file_with_elevated_permissions(file_path).await
            }
            Err(e) => Err(e).with_context(|| format!("Failed to read {:?}", file_path.as_ref()))?,
        }
    }

    /// Reads a file with elevated permissions when normal read fails due to permissions.
    async fn read_file_with_elevated_permissions<P: AsRef<Path>>(
        &self,
        file_path: P,
    ) -> Result<Vec<u8>, SQLiteError> {
        let temp_file = tempfile::NamedTempFile::new().map_err(|e| SQLiteError::Other(e.into()))?;
        let temp_path_str = file_fs::path_to_string(&temp_file)?;

        sudo::sudo_exec(
            "cp",
            &[
                "--preserve",
                "--no-dereference",
                &file_fs::path_to_string(&file_path)?,
                &temp_path_str,
            ],
            Some(
                format!(
                    "Create temporary copy of {:?} for backup creation",
                    file_path.as_ref()
                )
                .as_str(),
            ),
        )
        .await?;

        file_metadata::set_file_metadata(
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
            .with_context(|| format!("Failed to read {:?}", &temp_file))
            .map_err(SQLiteError::Other)
    }

    /// Inserts a backup entry into the database.
    async fn insert_backup_into_db(&self, b_file: StoreBackup) -> Result<(), SQLiteError> {
        let conn = &self.get_con().await?;

        conn.interact(move |conn| -> Result<(), SQLiteError> {
            db::prepare_connection(conn)?;
            let sql_stmt = "INSERT INTO backups (path, file_type, content, link_source, owner, permissions, checksum, date) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) ON CONFLICT(path) DO NOTHING";
            let mut stmt = conn.prepare(sql_stmt)?;

            stmt.execute(params![
                b_file.path,
                b_file.file_type,
                b_file.content,
                b_file.link_source,
                b_file.owner,
                b_file.permissions,
                b_file.checksum,
                b_file.date
            ])?;

            Ok(())
        })
            .await??;
        Ok(())
    }

    /// Removes a backup entry from the store database.
    ///
    /// # Arguments
    /// * `file_path` - The path of the file whose backup should be removed.
    ///
    /// # Returns
    /// * `Ok(())` if the backup is successfully removed.
    /// * `Err(SQLiteError)` if there's an error during the process.
    pub(crate) async fn remove_backup<P: AsRef<Path>>(
        &self,
        file_path: P,
    ) -> Result<(), SQLiteError> {
        let file_path_str = file_fs::path_to_string(&file_path)?;

        let conn = &self.get_con().await?;
        conn.interact(move |conn| -> Result<(), SQLiteError> {
            db::prepare_connection(conn)?;
            conn.execute(
                "DELETE FROM backups WHERE path = $1",
                params![file_path_str],
            )?;
            Ok(())
        })
        .await??;

        Ok(())
    }

    /// Checks if a backup of a file exists in the store database.
    ///
    /// # Arguments
    /// * `path` - The path of the file to check for a backup.
    ///
    /// # Returns
    /// * `Ok(bool)` - `true` if a backup exists, `false` otherwise.
    /// * `Err(SQLiteError)` if there's an error during the database operation.
    pub(crate) async fn check_backup_exists<P: AsRef<Path>>(
        &self,
        path: P,
    ) -> Result<bool, SQLiteError> {
        let path_str = file_fs::path_to_string(path)?;
        let store_path = self.path.clone();

        debug!(
            "Looking for backup of {} in {}",
            &path_str,
            &store_path.display()
        );

        let conn = &self.get_con().await?;

        let result = conn
            .interact(move |conn| -> Result<bool, SQLiteError> {
                db::prepare_connection(conn)?;
                let mut stmt = conn.prepare("SELECT path FROM backups where path = $1")?;

                match stmt.query_row(params![path_str], |row| row.get::<_, String>(0)) {
                    Ok(_) => {
                        debug!("Found backup of {} in {}", &path_str, &store_path.display());
                        Ok(true)
                    }
                    Err(e) if e == deadpool_sqlite::rusqlite::Error::QueryReturnedNoRows => {
                        debug!(
                            "Could not find backup of {} in {}",
                            &path_str,
                            &store_path.display()
                        );
                        Ok(false)
                    }
                    Err(e) => Err(SQLiteError::QueryError(e)),
                }
            })
            .await??;

        Ok(result)
    }

    /// Restores a backup from the store database to a specified location.
    ///
    /// # Arguments
    /// * `file_path` - The original path of the backed-up file.
    /// * `to` - The path where the backup should be restored.
    ///
    /// # Returns
    /// * `Ok(())` if the backup is successfully restored.
    /// * `Err(SQLiteError)` if there's an error during the restoration process.
    pub(crate) async fn restore_backup<P: AsRef<Path>>(
        &self,
        file_path: P,
        to: P,
    ) -> Result<(), SQLiteError> {
        // Safely handle the possibility that the path cannot be converted to a &str
        let file_path_str = file_fs::path_to_string(&file_path)?;

        let conn = &self.get_con().await?;

        let backup = self.fetch_backup_from_db(file_path_str, conn).await?;

        match backup.file_type.as_str() {
            "link" => self.restore_symlink_backup(backup, to).await?,
            "regular" => self.restore_regular_file_backup(backup, to).await?,
            _ => unreachable!(),
        }

        Ok(())
    }

    /// Fetches a backup entry from the database.
    async fn fetch_backup_from_db(
        &self,
        file_path_str: String,
        conn: &deadpool_sqlite::Object,
    ) -> Result<StoreBackup, SQLiteError> {
        conn.interact(move |conn| -> Result<StoreBackup, SQLiteError> {
            db::prepare_connection(conn)?;
            let mut stmt = conn.prepare(
                "SELECT path, file_type, content, link_source, owner, permissions, checksum, date FROM backups where path = $1"
            )?;

            Ok(stmt.query_row(params![file_path_str], |row| {
                Ok(StoreBackup {
                    path: row.get(0)?,
                    file_type: row.get(1)?,
                    content: row.get(2)?,
                    link_source: row.get(3)?,
                    owner: row.get(4)?,
                    permissions: row.get(5)?,
                    checksum: row.get(6)?,
                    date: row.get(7)?,
                })
            })?)
        })
        .await?
    }

    /// Restores a symlink backup.
    async fn restore_symlink_backup<P: AsRef<Path>>(
        &self,
        backup: StoreBackup,
        to: P,
    ) -> Result<(), SQLiteError> {
        match fs::symlink(backup.link_source.as_ref().unwrap(), &to).await {
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                sudo::sudo_exec(
                    "ln",
                    &[
                        "-sf",
                        backup.link_source.as_ref().unwrap(),
                        to.as_ref().to_str().unwrap(),
                    ],
                    None,
                )
                .await?;
            }
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("Failed to restore backup of {:?}", &backup.path))?
            }
        }

        let owner: Vec<&str> = backup.owner.split(':').collect();
        let permissions: Option<u32> = backup.permissions;

        self.set_file_metadata(to.as_ref(), owner, permissions, true).await?;
        Ok(())
    }

    /// Restores a regular file backup.
    async fn restore_regular_file_backup<P: AsRef<Path>>(
        &self,
        backup: StoreBackup,
        to: P,
    ) -> Result<(), SQLiteError> {
        let (write_dest, file) = self.prepare_write_destination(&to).await?;

        self.write_backup_content(file, &backup.content.clone().unwrap(), &write_dest)
            .await?;

        let owner: Vec<&str> = backup.owner.split(':').collect();
        let permissions: Option<u32> = backup.permissions;

        self.set_file_metadata(&write_dest, owner, permissions, false)
            .await?;

        if write_dest != to.as_ref() {
            self.move_file_with_sudo(&write_dest, to.as_ref()).await?;
        }

        Ok(())
    }

    /// Prepares the write destination for restoring a backup.
    async fn prepare_write_destination<P: AsRef<Path>>(
        &self,
        to: P,
    ) -> Result<(PathBuf, fs::File), SQLiteError> {
        let temp_file = tempfile::NamedTempFile::new().map_err(|e| SQLiteError::Other(e.into()))?;

        match fs::File::create(&to).await {
            Ok(f) => Ok((to.as_ref().to_path_buf(), f)),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                let temp_path = temp_file.path().to_path_buf();
                let file = fs::File::create(&temp_path)
                    .await
                    .map_err(|e| SQLiteError::Other(e.into()))?;
                Ok((temp_path, file))
            }
            Err(e) => {
                Err(e).with_context(|| format!("Failed to create file at {:?}", to.as_ref()))?
            }
        }
    }

    /// Writes the backup content to the specified file.
    async fn write_backup_content(
        &self,
        file: fs::File,
        content: &[u8],
        write_dest: &Path,
    ) -> Result<(), SQLiteError> {
        let mut writer = BufWriter::new(file);
        writer
            .write(content)
            .await
            .with_context(|| format!("Failed to write to file {:?}", write_dest))?;
        writer
            .flush()
            .await
            .context("Failed to flush write buffer")?;
        Ok(())
    }

    /// Sets file metadata (owner and permissions) for the restored file.
    async fn set_file_metadata(
        &self,
        path: &Path,
        owner: Vec<&str>,
        permissions: Option<u32>,
        is_symlink: bool,
    ) -> Result<(), SQLiteError> {
        file_metadata::set_file_metadata(
            path,
            file_metadata::FileMetadata {
                uid: Some(
                    owner[0]
                        .parse::<u32>()
                        .map_err(|e| SQLiteError::Other(e.into()))?,
                ),
                gid: Some(
                    owner[1]
                        .parse::<u32>()
                        .map_err(|e| SQLiteError::Other(e.into()))?,
                ),
                permissions,
                is_symlink,
                symlink_source: None,
                checksum: None,
            },
        )
        .await?;
        Ok(())
    }

    /// Moves a file using sudo when the destination requires elevated permissions.
    async fn move_file_with_sudo(&self, from: &Path, to: &Path) -> Result<(), SQLiteError> {
        sudo::sudo_exec(
            "cp",
            &["--preserve", from.to_str().unwrap(), to.to_str().unwrap()],
            None,
        )
        .await?;
        Ok(())
    }
}

//
// Tests

#[cfg(test)]
mod tests {
    use super::*;

    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    use tempfile::tempdir;

    use crate::store::tests::store_setup_helper;

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
        store
            .add_backup(&temp_path.path().join("foo.txt"))
            .await
            .map_err(|e| e.into_anyhow())?;
        fs::remove_file(temp_path.path().join("foo.txt")).await?;
        assert!(!temp_path.path().join("foo.txt").exists());

        // Restore file
        store
            .restore_backup(
                &temp_path.path().join("foo.txt"),
                &temp_path.path().join("foo.txt"),
            )
            .await
            .map_err(|e| e.into_anyhow())?;
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
