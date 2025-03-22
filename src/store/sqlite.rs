use crate::config::DotdeployConfig;
use crate::store::sqlite_backups::StoreBackup;
use crate::store::sqlite_checksums::{StoreSourceFileChecksum, StoreTargetFileChecksum};
use crate::store::sqlite_files::StoreFile;
use crate::store::sqlite_modules::StoreModule;
use crate::store::{Store, create_system_dir, create_user_dir};
use crate::utils::sudo::PrivilegeManager;
use crate::utils::{FileUtils, file_fs};
use color_eyre::eyre::WrapErr;
use color_eyre::{Result, Section};
use sqlx::sqlite;
use std::collections::HashSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
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
    pub(crate) privilege_manager: Arc<PrivilegeManager>,
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
    pub(crate) fn new(
        pool: sqlite::SqlitePool,
        path: PathBuf,
        system: bool,
        pm: Arc<PrivilegeManager>,
    ) -> Self {
        SQLiteStore {
            pool,
            path,
            system,
            privilege_manager: pm,
        }
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
/// # Errors
/// Returns an error if:
/// - Directory creation fails
/// - Database initialization fails
/// - Connection pool setup fails
/// - Setting file permissions fails
pub(crate) async fn init_sqlite_store(
    config: &DotdeployConfig,
    system: bool,
    pm: Arc<PrivilegeManager>,
) -> Result<SQLiteStore> {
    // Create the directory if it doesn't exist
    let path = match system {
        true => {
            create_system_dir(&config.system_store_path, Arc::clone(&pm)).await?;
            &config.system_store_path
        }
        false => {
            create_user_dir(&config.user_store_path, Arc::clone(&pm)).await?;
            &config.user_store_path
        }
    };

    // Create the connection pool
    let pool = init_pool(config, path)
        .await
        .wrap_err_with(|| format!("Failed to initialize user store in {}", &path.display()))
        .suggestion(format!(
            "Ensure that {} exists and you have read and write permissions to it",
            &path.display()
        ))?;

    // Create a new Store instance and initialize it
    let store = SQLiteStore::new(pool.0, pool.1, system, pm);
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
/// # Errors
/// Returns an error if database creation, migration, or pool setup fails.
async fn init_pool(config: &DotdeployConfig, path: &Path) -> Result<(sqlite::SqlitePool, PathBuf)> {
    // Set the full path for the SQLite database file
    let path = path.join("store.sqlite");

    let database_url = format!("sqlite://{}", file_fs::path_to_string(&path)?);
    let pool_timeout = std::time::Duration::from_secs(30);
    // We set the number of connections to the number of logical CPUs, with an upper limit of 64
    let max_connections = if config.deploy_sys_files {
        // If we deploy non-user files, we will need to connect to two databases. Thus, use half
        // of the maximum number of connections per pool.
        u32::try_from(std::cmp::min(num_cpus::get(), 64) / 2)
            .wrap_err("Failed to convert usize to u32")?
    } else {
        u32::try_from(std::cmp::min(num_cpus::get(), 64))
            .wrap_err("Failed to convert usize to u32")?
    };

    let connection_options = sqlite::SqliteConnectOptions::from_str(&database_url)?
        .create_if_missing(true)
        .journal_mode(sqlite::SqliteJournalMode::Wal)
        .synchronous(sqlite::SqliteSynchronous::Normal)
        .optimize_on_close(true, 1000)
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
        let file_utils = FileUtils::new(Arc::clone(&self.privilege_manager));
        let file_path_str = file_fs::path_to_string(&file_path)?;
        let metadata = file_utils.get_file_metadata(&file_path).await?;

        let b_file: StoreBackup = if metadata.is_symlink {
            self.create_symlink_backup(&file_path_str, &metadata)?
        } else {
            self.create_regular_file_backup(&file_path, &file_path_str, metadata)
                .await?
        };

        self.insert_backup_into_db(b_file).await
    }

    async fn add_dummy_backup<P: AsRef<Path>>(&self, file_path: P) -> Result<()> {
        let file_path_str = file_fs::path_to_string(&file_path)?;

        self.insert_backup_into_db(StoreBackup {
            path: file_path_str,
            file_type: "dummy".to_string(),
            content: None,
            link_source: None,
            owner: "9999:9999".to_string(),
            permissions: None,
            checksum: None,
            date: chrono::offset::Utc::now(),
        })
        .await
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

    #[instrument(skip(file_path, to))]
    async fn restore_backup<P: AsRef<Path>>(&self, file_path: P, to: P) -> Result<()> {
        // Safely handle the possibility that the path cannot be converted to a &str
        let file_path_str = file_fs::path_to_string(&file_path)?;

        let backup = self.fetch_backup_from_db(file_path_str).await?;

        match backup.file_type.as_str() {
            "link" => self.restore_symlink_backup(backup, to).await?,
            "regular" => self.restore_regular_file_backup(backup, to).await?,
            "dummy" => (),
            _ => unreachable!(),
        }

        Ok(())
    }

    // --
    // * Modules

    async fn add_module(&self, module: &StoreModule) -> Result<()> {
        let depends_json = match &module.depends {
            Some(dep) => Some(serde_json::to_string(dep)?),
            None => None,
        };

        sqlx::query!(
            r#"
INSERT INTO modules (name, location, user, reason, depends, date)
VALUES (?1, ?2, ?3, ?4, ?5, ?6)
ON CONFLICT(name)
DO UPDATE SET
  name = excluded.name,
  location = excluded.location,
  user = excluded.user,
  reason = CASE
             WHEN modules.reason = 'automatic' AND excluded.reason = 'manual'
             THEN excluded.reason
             ELSE modules.reason
           END,
  depends = excluded.depends,
  date = excluded.date
            "#,
            module.name,
            module.location,
            module.user,
            module.reason,
            depends_json,
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

        // Fetch the row as an anonymous struct
        let row = sqlx::query!(
            r#"
SELECT name, location, user, reason, depends, date as "date: chrono::DateTime<chrono::Utc>"
FROM modules
WHERE name = ?1
            "#,
            name
        )
        .fetch_one(&self.pool)
        .await?;

        // Deserialize the depends JSON string
        let depends = match row.depends {
            Some(json_str) if json_str == "[]" => None,
            Some(json_str) => serde_json::from_str(&json_str)?,
            None => None,
        };

        Ok(StoreModule {
            name: row.name,
            location: row.location,
            user: row.user,
            reason: row.reason,
            depends,
            date: row.date,
        })
    }

    async fn get_all_modules(&self) -> Result<Vec<StoreModule>> {
        let rows = sqlx::query!(
            r#"
SELECT name, location, user, reason, depends, date as "date: chrono::DateTime<chrono::Utc>"
FROM modules
            "#
        )
        .fetch_all(&self.pool)
        .await?;

        let modules = rows
            .into_iter()
            .map(|row| {
                // Deserialize the depends JSON string for each row
                let depends = match row.depends {
                    Some(json_str) if json_str == "[]" => None,
                    Some(json_str) => serde_json::from_str(&json_str).unwrap(),
                    None => None,
                };

                StoreModule {
                    name: row.name,
                    location: row.location,
                    user: row.user,
                    reason: row.reason,
                    depends,
                    date: row.date,
                }
            })
            .collect();

        Ok(modules)
    }

    // --
    // * Files

    #[instrument(skip(filename))]
    async fn get_file<P: AsRef<Path>>(&self, filename: P) -> Result<StoreFile> {
        let filename_str = file_fs::path_to_string(filename)?;
        debug!("getting {}", filename_str);
        let result = sqlx::query_as!(StoreFile,
        r#"
SELECT files.source, files.source_checksum, files.target, files.target_checksum, files.operation, files.user, files.date as "date: chrono::DateTime<chrono::Utc>", modules.name AS module
FROM files
INNER JOIN modules ON files.module_id = modules.id
WHERE files.target = ?1
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
INSERT INTO files (module_id, source, source_checksum, target, target_checksum, operation, user, date)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
ON CONFLICT(target)
DO UPDATE SET
  module_id = excluded.module_id,
  source = excluded.source,
  source_checksum = excluded.source_checksum,
  target_checksum = excluded.target_checksum,
  operation = excluded.operation,
  user = excluded.user,
  date = excluded.date
            "#,
            module_id.id,
            file.source,
            file.source_checksum,
            file.target,
            file.target_checksum,
            file.operation,
            file.user,
            file.date
        ).execute(&self.pool).await?;

        Ok(())
    }

    async fn remove_file<S: AsRef<str>>(&self, file: S) -> Result<()> {
        let file = file.as_ref().to_owned();
        sqlx::query!("DELETE FROM files WHERE target = ?1", file)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn get_all_files<S: AsRef<str>>(&self, module: S) -> Result<Vec<StoreFile>> {
        let module = module.as_ref().to_owned();

        let result = sqlx::query_as!(StoreFile,
    r#"
SELECT files.source, files.source_checksum, files.target, files.target_checksum, files.operation, files.user, files.date as "date: chrono::DateTime<chrono::Utc>", modules.name AS module
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

        let result = sqlx::query!("SELECT target FROM files WHERE target = ?1", path_str)
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
    ) -> Result<StoreSourceFileChecksum> {
        // Convert the path to a string, handling potential conversion errors
        let filename_str = file_fs::path_to_string(filename)?;

        // Retrieve the checksum for the source file
        let file = sqlx::query!(
            "SELECT source, source_checksum FROM files WHERE target = ?1",
            filename_str
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(file.map_or(StoreSourceFileChecksum::new(None, None), |f| {
            StoreSourceFileChecksum::new(f.source, f.source_checksum)
        }))
    }

    async fn get_target_checksum<P: AsRef<Path>>(
        &self,
        filename: P,
    ) -> Result<StoreTargetFileChecksum> {
        // Convert the path to a string, handling potential conversion errors
        let filename_str = file_fs::path_to_string(filename)?;

        // Retrieve the checksum for the source file
        let file = sqlx::query_as!(
            StoreTargetFileChecksum,
            "SELECT target, target_checksum FROM files WHERE target = ?1",
            filename_str
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(
            file.map_or(StoreTargetFileChecksum::new(filename_str, None), |f| {
                StoreTargetFileChecksum::new(f.target, f.target_checksum)
            }),
        )
    }

    async fn get_all_source_checksums(&self) -> Result<Vec<StoreSourceFileChecksum>> {
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

    async fn get_all_target_checksums(&self) -> Result<Vec<StoreTargetFileChecksum>> {
        let res = sqlx::query_as!(
            StoreTargetFileChecksum,
            "SELECT target, target_checksum FROM files"
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(res)
    }

    // --
    // * Packages

    async fn add_package<S: AsRef<str>>(&self, module: S, package: S) -> Result<()> {
        let module = module.as_ref();
        let package = package.as_ref();

        // Retrieve the ID of the module
        let module_id = sqlx::query!("SELECT id FROM modules WHERE name = ?1", module)
            .fetch_one(&self.pool)
            .await?;

        sqlx::query!(
            r#"
INSERT OR IGNORE INTO packages (module_id, name)
VALUES (?1, ?2)
            "#,
            module_id.id,
            package,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn remove_package<S: AsRef<str>>(&self, module: S, package: S) -> Result<()> {
        let module = module.as_ref();
        let package = package.as_ref();

        // Retrieve the ID of the module
        let module_id = sqlx::query!("SELECT id FROM modules WHERE name = ?1", module)
            .fetch_one(&self.pool)
            .await?;

        sqlx::query!(
            "DELETE FROM packages WHERE module_id = ?1 AND name = ?2",
            module_id.id,
            package,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_all_module_packages<S: AsRef<str>>(&self, module: S) -> Result<Vec<String>> {
        let module = module.as_ref();

        // Retrieve the ID of the module
        let module_id = sqlx::query!("SELECT id FROM modules WHERE name = ?1", module)
            .fetch_one(&self.pool)
            .await?;

        let rows = sqlx::query!(
            "SELECT name FROM packages WHERE module_id = ?1",
            module_id.id
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|r| r.name).collect())
    }

    async fn get_all_other_module_packages<S: AsRef<str>>(&self, module: S) -> Result<Vec<String>> {
        let module = module.as_ref();

        // Retrieve the ID of the module
        let module_id = sqlx::query!("SELECT id FROM modules WHERE name = ?1", module)
            .fetch_one(&self.pool)
            .await?;

        let rows = sqlx::query!(
            "SELECT name FROM packages WHERE module_id != ?1",
            module_id.id
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|r| r.name).collect())
    }

    async fn get_all_packages(&self) -> Result<Vec<String>> {
        let rows = sqlx::query!("SELECT name FROM packages")
            .fetch_all(&self.pool)
            .await?;

        // Filter duplicates and return packaages
        let mut seen = HashSet::new();
        Ok(rows
            .into_iter()
            .filter(|x| seen.insert(x.name.clone()))
            .map(|x| x.name)
            .collect::<Vec<_>>())
    }
}

// -------------------------------------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::tests::pm_setup;
    use color_eyre::eyre::eyre;
    use tempfile::tempdir;

    pub(crate) async fn store_setup_helper(op_tye: &str) -> Result<SQLiteStore> {
        let temp_dir = tempdir()?;
        let (_tx, pm) = tests::pm_setup()?;

        // init_user_store(Some(temp_dir.into_path()))
        let config = DotdeployConfig {
            user_store_path: temp_dir.into_path(),
            ..Default::default()
        };
        // Initialize the user store, which sets up the database and tables
        let pool = init_sqlite_store(&config, false, pm).await?;

        // Insert a module
        let test_module = StoreModule::new(
            "test".to_string(),
            "/testpath".to_string(),
            Some("user".to_string()),
            "manual".to_string(),
            None,
            chrono::offset::Utc::now(),
        );

        pool.add_module(&test_module).await?;

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
                        ));
                    }
                },
                match op_tye {
                    "link" => Some(format!("source_checksum{}", i)),
                    "copy" => Some(format!("source_checksum{}", i)),
                    "create" => None,
                    _ => {
                        return Err(eyre!(
                            "Invalid 'which' parameter. Must be either 'link', 'copy' or 'create'."
                        ));
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
                        ));
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
