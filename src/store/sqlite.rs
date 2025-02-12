use crate::config::DotdeployConfig;
use crate::store::sqlite_backups::StoreBackup;
use crate::store::sqlite_checksums::{StoreDestFileChecksum, StoreSourceFileChecksum};
use crate::store::sqlite_files::StoreFile;
use crate::store::sqlite_modules::StoreModule;
use crate::store::{create_system_dir, create_user_dir, Store};
use crate::utils::{file_fs, file_metadata};
use color_eyre::eyre::WrapErr;
use color_eyre::{Result, Section};
use sqlx::sqlite;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tokio::fs;
use tracing::{debug, instrument};

/// Representation of the store database
#[derive(Clone, Debug)]
pub(crate) struct SQLiteStore {
    /// SQLite connection pool
    pub(crate) pool: sqlite::SqlitePool,
    /// Store location
    pub(crate) path: PathBuf,
    /// Indicates whether this is a system-wide store (true) or user-specific store (false)
    pub(crate) system: bool,
}

impl SQLiteStore {
    /// Creates a new [`SQLiteStore`] instance.
    ///
    /// # Arguments
    /// * `pool` - A [`sqlite::SqlitePool`].
    /// * `path` - The path where the store database will be created.
    /// * `system` - A boolean indicating whether this is a system-wide store (true) or
    ///   user-specific store (false).
    ///
    /// # Returns
    /// A new [`SQLiteStore`] instance with the specified path and system flag.
    pub(crate) fn new(pool: sqlite::SqlitePool, path: PathBuf, system: bool) -> Self {
        SQLiteStore { pool, path, system }
    }
}

// -------------------------------------------------------------------------------------------------
// Helper functions for SQLiteStore
// -------------------------------------------------------------------------------------------------

/// Initialize a [`SQLiteStore`].
///
/// This function creates and initializes a SQLite database for storing user or system-wide
/// dotdeploy data. Depending on the value of `system`, the database is either created in the user's
/// configuration directory as specified in [`field@DotdeployConfig::user_store_path`] or the
/// directory specified in [`field@DotdeployConfig::system_store_path`]. After creation, the system
/// database file permissions are set to be readable and writable by all users (0o666). This is not
/// the case for the user database.
///
/// # Arguments
/// * `config` - The DotdeployConfig containing configuration settings including the store path
/// * `system` - A boolean indicating whether this is a system-wide store (true) or user-specific
///   store (false).
///
/// # Returns
/// Returns `Ok(SQLiteStore)` if the store is successfully initialized, or an error if:
/// - Directory creation fails
/// - Database initialization fails
/// - Connection pool setup fails
/// - Setting file permissions fails
pub(crate) async fn init_sqlite_store(
    config: &DotdeployConfig,
    system: bool,
) -> Result<SQLiteStore> {
    // Create the directory if it doesn't exist
    let path = match system {
        true => {
            create_system_dir(&config.system_store_path).await?;
            &config.system_store_path
        }
        false => {
            create_user_dir(&config.user_store_path).await?;
            &config.user_store_path
        }
    };

    // Create the connection pool
    let pool = init_pool(config, &path)
        .await
        .wrap_err_with(|| format!("Failed to initialize user store in {}", &path.display()))
        .suggestion(format!(
            "Ensure that {} exists and you have read and write permissions to it",
            &path.display()
        ))?;

    // Create a new Store instance and initialize it
    let store = SQLiteStore::new(pool.0, pool.1, system);
    if store.is_system() {
        // Set permissions for the store file to be readable and writable by all users
        fs::set_permissions(&store.path(), std::fs::Permissions::from_mode(0o666)).await?;
    }

    Ok(store)
}

/// Initializes and configures a SQLite connection pool for the store database.
///
/// This function:
/// - Creates a SQLite database file if it doesn't exist
/// - Configures connection options including WAL journal mode and synchronous settings
/// - Sets up a connection pool sized based on CPU cores
/// - Runs any pending database migrations
///
/// # Arguments
/// * `config` - Configuration settings including system file deployment options
/// * `path` - Base directory path where the SQLite database file will be created
///
/// # Returns
/// * `Ok(SqlitePool)` - Configured and initialized connection pool
/// * `Err` - If database creation, migration, or pool setup fails
async fn init_pool(
    config: &DotdeployConfig,
    path: &PathBuf,
) -> Result<(sqlite::SqlitePool, PathBuf)> {
    // Set the full path for the SQLite database file
    let path = path.join("store.sqlite");

    let database_url = format!("sqlite://{}", file_fs::path_to_string(&path)?);
    let pool_timeout = std::time::Duration::from_secs(30);
    // We set the number of connections to 4 times the number of physical CPUs
    let max_connections = if config.deploy_sys_files {
        // If we deploy non-user files, we will need to connect to two databases. Thus, use half
        // of the maximum number of connections per pool.
        u32::try_from(num_cpus::get_physical() * 2).wrap_err("Failed to convert usize to u32")?
    } else {
        u32::try_from(num_cpus::get_physical() * 4).wrap_err("Failed to convert usize to u32")?
    };

    let connection_options = sqlite::SqliteConnectOptions::from_str(&database_url)?
        .create_if_missing(true)
        .journal_mode(sqlite::SqliteJournalMode::Wal)
        .synchronous(sqlite::SqliteSynchronous::Normal)
        .busy_timeout(pool_timeout);

    let pool = sqlite::SqlitePoolOptions::new()
        .max_connections(max_connections)
        .connect_with(connection_options)
        .await?;

    // Create the necessary tables, if they do not exist already
    sqlx::migrate!("./db")
        .run(&pool)
        .await
        .wrap_err("Failed to initialize store database")?;

    // Return the initialized pool
    Ok((pool, path))
}

// -------------------------------------------------------------------------------------------------
// Store impl for SQLiteStore
// -------------------------------------------------------------------------------------------------

impl Store for SQLiteStore {
    fn path(&self) -> &PathBuf {
        &self.path
    }

    fn is_system(&self) -> bool {
        self.system
    }

    // --
    // * Backups

    #[instrument(skip(file_path))]
    async fn add_backup<P: AsRef<Path>>(&self, file_path: P) -> Result<()> {
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

    async fn remove_backup<P: AsRef<Path>>(&self, file_path: P) -> Result<()> {
        let file_path_str = file_fs::path_to_string(&file_path)?;

        sqlx::query!("DELETE FROM backups WHERE path = ?1", file_path_str)
            .execute(&self.pool)
            .await
            .wrap_err_with(|| format!("Failed to remove backup of {}", file_path_str))?;

        Ok(())
    }

    #[instrument(skip(path))]
    async fn check_backup_exists<P: AsRef<Path>>(&self, path: P) -> Result<bool> {
        let path_str = file_fs::path_to_string(path)?;
        let store_path = self.path.clone();

        debug!(
            "Looking for backup of {} in {}",
            &path_str,
            &store_path.display()
        );
        let result = sqlx::query!("SELECT path FROM backups where path = ?1", path_str)
            .fetch_optional(&self.pool)
            .await?;
        match result {
            Some(_) => {
                debug!("Found backup of {} in {}", &path_str, &store_path.display());
                Ok(true)
            }
            None => {
                debug!(
                    "Could not find backup of {} in {}",
                    &path_str,
                    &store_path.display()
                );
                Ok(false)
            }
        }
    }

    async fn restore_backup<P: AsRef<Path>>(&self, file_path: P, to: P) -> Result<()> {
        // Safely handle the possibility that the path cannot be converted to a &str
        let file_path_str = file_fs::path_to_string(&file_path)?;

        let backup = self.fetch_backup_from_db(file_path_str).await?;

        match backup.file_type.as_str() {
            "link" => self.restore_symlink_backup(backup, to).await?,
            "regular" => self.restore_regular_file_backup(backup, to).await?,
            _ => unreachable!(),
        }

        Ok(())
    }

    // --
    // * Modules

    async fn add_module(&self, module: StoreModule) -> Result<()> {
        sqlx::query!(
            r#"
INSERT INTO modules (name, location, user, reason, depends, date)
VALUES (?1, ?2, ?3, ?4, ?5, ?6)
ON CONFLICT(name) DO NOTHING
            "#,
            module.name,
            module.location,
            module.user,
            module.reason,
            module.depends,
            module.date
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn remove_module<S: AsRef<str>>(&self, module: S) -> Result<()> {
        let module = module.as_ref().to_owned();
        sqlx::query!("DELETE FROM modules WHERE name = ?1", module)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn get_module<S: AsRef<str>>(&self, name: S) -> Result<StoreModule> {
        let name = name.as_ref().to_owned();

        let result = sqlx::query_as!(
            StoreModule,
            r#"
SELECT name, location, user, reason, depends, date as "date: chrono::DateTime<chrono::Utc>"
FROM modules
WHERE name = ?1
            "#,
            name
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(result)
    }

    async fn get_all_modules(&self) -> Result<Vec<StoreModule>> {
        let result = sqlx::query_as!(
            StoreModule,
            r#"
SELECT name, location, user, reason, depends, date as "date: chrono::DateTime<chrono::Utc>"
FROM modules
            "#
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(result)
    }

    // --
    // * Files

    async fn get_file<P: AsRef<Path>>(&self, filename: P) -> Result<StoreFile> {
        let filename_str = file_fs::path_to_string(filename)?;

        let result = sqlx::query_as!(StoreFile,
        r#"
SELECT files.source, files.source_checksum, files.destination, files.destination_checksum, files.operation, files.user, files.date as "date: chrono::DateTime<chrono::Utc>", modules.name AS module
FROM files
INNER JOIN modules ON files.module_id = modules.id
WHERE files.destination = ?1
        "#, filename_str).fetch_one(&self.pool).await?;

        Ok(result)
    }

    async fn add_file(&self, file: StoreFile) -> Result<()> {
        // Retrieve the ID of the module
        let module_id = sqlx::query!("SELECT id FROM modules WHERE name = ?1", file.module)
            .fetch_one(&self.pool)
            .await?;

        sqlx::query!(
            r#"
INSERT INTO files (module_id, source, source_checksum, destination, destination_checksum, operation, user, date)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
ON CONFLICT(destination)
DO UPDATE SET
  module_id = excluded.module_id,
  source = excluded.source,
  source_checksum = excluded.source_checksum,
  destination_checksum = excluded.destination_checksum,
  operation = excluded.operation,
  user = excluded.user,
  date = excluded.date
            "#,
            module_id.id,
            file.source,
            file.source_checksum,
            file.destination,
            file.destination_checksum,
            file.operation,
            file.user,
            file.date
        ).execute(&self.pool).await?;

        Ok(())
    }

    async fn remove_file<S: AsRef<str>>(&self, file: S) -> Result<()> {
        let file = file.as_ref().to_owned();
        sqlx::query!("DELETE FROM files WHERE destination = ?1", file)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn get_all_files<S: AsRef<str>>(&self, module: S) -> Result<Vec<StoreFile>> {
        let module = module.as_ref().to_owned();

        let result = sqlx::query_as!(StoreFile,
    r#"
SELECT files.source, files.source_checksum, files.destination, files.destination_checksum, files.operation, files.user, files.date as "date: chrono::DateTime<chrono::Utc>", modules.name AS module
FROM files
INNER JOIN modules ON files.module_id = modules.id
WHERE modules.name = ?1
    "#, module).fetch_all(&self.pool).await?;

        Ok(result)
    }
    #[instrument(skip(path))]
    async fn check_file_exists<P: AsRef<Path>>(&self, path: P) -> Result<bool> {
        let path_str = file_fs::path_to_string(path)?;
        let store_path = self.path.clone();

        debug!("Looking for {} in {}", &path_str, &self.path.display());

        let result = sqlx::query!(
            "SELECT destination FROM files WHERE destination = ?1",
            path_str
        )
        .fetch_optional(&self.pool)
        .await?;
        match result {
            Some(_) => {
                debug!("Found {} in {}", &path_str, &store_path.display());
                Ok(true)
            }
            None => {
                debug!("Could not find {} in {}", &path_str, &store_path.display());
                Ok(false)
            }
        }
    }

    // --
    // * Checksums

    async fn get_source_checksum<P: AsRef<Path>>(
        &self,
        filename: P,
    ) -> Result<Option<StoreSourceFileChecksum>> {
        // Convert the path to a string, handling potential conversion errors
        let filename_str = file_fs::path_to_string(filename)?;

        // Retrieve the checksum for the source file
        let file = sqlx::query_as!(
            StoreSourceFileChecksum,
            "SELECT source, source_checksum FROM files WHERE destination = ?1",
            filename_str
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(file)
    }

    async fn get_destination_checksum<P: AsRef<Path>>(
        &self,
        filename: P,
    ) -> Result<Option<StoreDestFileChecksum>> {
        // Convert the path to a string, handling potential conversion errors
        let filename_str = file_fs::path_to_string(filename)?;

        // Retrieve the checksum for the source file
        let file = sqlx::query_as!(
            StoreDestFileChecksum,
            "SELECT destination, destination_checksum FROM files WHERE destination = ?1",
            filename_str
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(file)
    }

    async fn get_all_src_checksums(&self) -> Result<Vec<StoreSourceFileChecksum>> {
        let res = sqlx::query_as!(
            StoreSourceFileChecksum,
            "SELECT source, source_checksum FROM files"
        )
        .fetch_all(&self.pool)
        .await?;

        // Filter out records where both source and source_checksum are None
        Ok(res
            .into_iter()
            .filter(|r| r.source.is_some() || r.source_checksum.is_some())
            .collect())
    }

    async fn get_all_dest_checksums(&self) -> Result<Vec<StoreDestFileChecksum>> {
        let res = sqlx::query_as!(
            StoreDestFileChecksum,
            "SELECT destination, destination_checksum FROM files"
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(res)
    }
}

// -------------------------------------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use color_eyre::eyre::eyre;
    use tempfile::tempdir;

    pub(crate) async fn store_setup_helper(op_tye: &str) -> Result<SQLiteStore> {
        let temp_dir = tempdir()?;

        // init_user_store(Some(temp_dir.into_path()))
        let config = DotdeployConfig {
            user_store_path: temp_dir.into_path(),
            ..Default::default()
        };
        // Initialize the user store, which sets up the database and tables
        let pool = init_sqlite_store(&config, false).await?;

        // Insert a module
        let test_module = StoreModule::new(
            "test".to_string(),
            "/testpath".to_string(),
            Some("user".to_string()),
            "manual".to_string(),
            None,
            chrono::offset::Utc::now(),
        );

        pool.add_module(test_module).await?;

        for i in 0..5 {
            let local_time = chrono::offset::Utc::now();
            let test_file = StoreFile::new(
                "test".to_string(),
                match op_tye {
                    "link" => Some(format!("/dotfiles/foo{}.txt", i)),
                    "copy" => Some(format!("/dotfiles/foo{}.txt", i)),
                    "create" => None,
                    _ => {
                        return Err(eyre!(
                            "Invalid 'which' parameter. Must be either 'link', 'copy' or 'create'."
                        ))
                    }
                },
                match op_tye {
                    "link" => Some(format!("source_checksum{}", i)),
                    "copy" => Some(format!("source_checksum{}", i)),
                    "create" => None,
                    _ => {
                        return Err(eyre!(
                            "Invalid 'which' parameter. Must be either 'link', 'copy' or 'create'."
                        ))
                    }
                },
                format!("/home/foo{}.txt", i),
                Some(format!("dest_checksum{}", i)),
                match op_tye {
                    "link" => "link".to_string(),
                    "copy" => "copy".to_string(),
                    "create" => "create".to_string(),
                    _ => {
                        return Err(eyre!(
                            "Invalid 'which' parameter. Must be either 'link', 'copy' or 'create'."
                        ))
                    }
                },
                Some("user".to_string()),
                local_time,
            );

            pool.add_file(test_file).await?;
        }

        Ok(pool)
    }
}
