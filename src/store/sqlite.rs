use crate::config::DotdeployConfig;
use crate::modules::messages::CommandMessage;
use crate::phases::task::PhaseTask;
use crate::store::sqlite_backups::StoreBackupBuilder;
use crate::store::sqlite_checksums::{StoreSourceFileChecksum, StoreTargetFileChecksum};
use crate::store::sqlite_files::StoreFile;
use crate::store::sqlite_modules::StoreModule;
use crate::store::sqlite_modules::StoreModuleBuilder;
use crate::store::{Store, create_user_dir};
use crate::utils::FileUtils;
use crate::utils::common::{bytes_to_os_str, os_str_to_bytes};
use crate::utils::sudo::PrivilegeManager;
use color_eyre::eyre::{WrapErr, eyre};
use color_eyre::{Result, Section};
use sqlx::sqlite;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Representation of the store database
#[derive(Clone, Debug)]
pub(crate) struct SQLiteStore {
    /// SQLite connection pool
    pub(crate) pool: sqlite::SqlitePool,
    /// Store location
    pub(crate) path: PathBuf,
    pub(crate) privilege_manager: Arc<PrivilegeManager>,
}

impl SQLiteStore {
    /// Creates a new [`SQLiteStore`] instance.
    ///
    /// # Arguments
    /// * `pool` - A [`sqlite::SqlitePool`].
    /// * `path` - The path where the store database will be created.
    ///
    /// # Returns
    /// A new [`SQLiteStore`] instance with the specified path.
    pub(crate) fn new(pool: sqlite::SqlitePool, path: PathBuf, pm: Arc<PrivilegeManager>) -> Self {
        SQLiteStore {
            pool,
            path,
            privilege_manager: pm,
        }
    }
}

// -------------------------------------------------------------------------------------------------
// Helper functions for SQLiteStore
// -------------------------------------------------------------------------------------------------

/// Initialize a [`SQLiteStore`].
///
/// This function creates and initializes a SQLite database for storing user dotdeploy data. The
/// database is created in the user's configuration directory as specified in
/// [`field@DotdeployConfig::user_store_path`].
///
/// # Arguments
/// * `config` - The DotdeployConfig containing configuration settings including the store path
///
/// # Errors
/// Returns an error if:
/// - Directory creation fails
/// - Database initialization fails
/// - Connection pool setup fails
pub(crate) async fn init_sqlite_store(
    config: &DotdeployConfig,
    pm: Arc<PrivilegeManager>,
) -> Result<SQLiteStore> {
    // Create the directory if it doesn't exist
    let path = {
        create_user_dir(&config.user_store_path, Arc::clone(&pm)).await?;
        &config.user_store_path
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
    let store = SQLiteStore::new(pool.0, pool.1, pm);

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
    let database_url = format!(
        "sqlite://{}",
        path.to_str()
            .ok_or_else(|| eyre!("{:?} is not valid UTF-8", path))?
    );

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

    // --
    // * Backups

    async fn add_backup<P: AsRef<Path>>(&self, file_path: P) -> Result<()> {
        let file_utils = FileUtils::new(Arc::clone(&self.privilege_manager));

        let metadata = file_utils
            .get_file_metadata(&file_path)
            .await
            .wrap_err_with(|| {
                format!(
                    "Failed to get metadata for {}",
                    &file_path.as_ref().display()
                )
            })?;

        let b_file = match metadata.is_symlink {
            true => self
                .create_symlink_backup(&file_path, &metadata)
                .wrap_err("Failed to create symlink backup")?,
            false => self
                .create_regular_file_backup(&file_path, metadata)
                .await
                .wrap_err("Failed to create regular file backup")?,
        };

        self.insert_backup_into_db(b_file)
            .await
            .wrap_err("Failed to insert backup into database")
    }

    async fn add_dummy_backup<P: AsRef<Path>>(&self, file_path: P) -> Result<()> {
        self.insert_backup_into_db(
            StoreBackupBuilder::default()
                .with_path(file_path.as_ref().to_string_lossy())
                .with_path_u8(os_str_to_bytes(file_path.as_ref()))
                .with_file_type("dummy")
                .with_content(None)
                .with_link_source(None)
                .with_link_source_u8(None)
                .with_owner("9999:9999".to_string())
                .with_permissions(None)
                .with_checksum(None)
                .with_date(chrono::offset::Utc::now())
                .build()?,
        )
        .await
    }

    async fn remove_backup<P: AsRef<Path>>(&self, file_path: P) -> Result<()> {
        let file_path_u8 = os_str_to_bytes(file_path.as_ref());

        sqlx::query!("DELETE FROM backups WHERE path_u8 = ?1", file_path_u8)
            .execute(&self.pool)
            .await
            .wrap_err_with(|| {
                format!(
                    "Failed to remove backup for '{}'",
                    file_path.as_ref().display()
                )
            })?;

        Ok(())
    }

    async fn check_backup_exists<P: AsRef<Path>>(&self, file_path: P) -> Result<bool> {
        let file_path_u8 = os_str_to_bytes(file_path.as_ref());

        debug!(
            "Checking backup existence for {} in {}",
            file_path.as_ref().display(),
            self.path().display()
        );

        sqlx::query!("SELECT path FROM backups WHERE path_u8 = ?1", file_path_u8)
            .fetch_optional(&self.pool)
            .await
            .map(|result| result.is_some())
            .wrap_err_with(|| {
                format!(
                    "Failed to check backup existence for '{}'",
                    file_path.as_ref().display()
                )
            })
    }

    async fn restore_backup<P: AsRef<Path>>(&self, file_path: P, to: P) -> Result<()> {
        let backup = self
            .fetch_backup_from_db(&file_path)
            .await
            .wrap_err_with(|| format!("Backup not found for {}", file_path.as_ref().display()))?;

        let file_utils = FileUtils::new(Arc::clone(&self.privilege_manager));
        file_utils
            .ensure_dir_exists(
                &to.as_ref().parent().ok_or_else(|| {
                    eyre!("Failed to get parent dir of {}", to.as_ref().display())
                })?,
            )
            .await?;

        let result = match backup.file_type.as_str() {
            "link" => {
                info!(
                    "Restoring {} backup to {}",
                    file_path.as_ref().display(),
                    to.as_ref().display()
                );

                self.restore_symlink_backup(&backup, &to).await
            }
            "regular" => {
                info!(
                    "Restoring {} backup to {}",
                    file_path.as_ref().display(),
                    to.as_ref().display()
                );

                self.restore_regular_file_backup(&backup, &to).await
            }
            "dummy" => Ok(()),
            invalid => Err(eyre!(
                "Invalid backup type '{}' for {}",
                invalid,
                file_path.as_ref().display()
            )),
        };

        // Validate checksum for regular files
        if backup.file_type == "regular" {
            let restored_checksum = file_utils
                .calculate_sha256_checksum(&to)
                .await
                .wrap_err_with(|| {
                    format!("Failed to verify restored file {}", to.as_ref().display())
                })?;

            if let Some(expected_checksum) = backup.checksum {
                if restored_checksum != expected_checksum {
                    return Err(eyre!(
                        "Checksum mismatch for restored file {}: expected {}, got {}",
                        to.as_ref().display(),
                        expected_checksum,
                        restored_checksum
                    ));
                }
            } else {
                return Err(eyre!(
                    "Missing checksum in backup record for {}",
                    to.as_ref().display()
                ));
            }
        }

        result
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
INSERT INTO modules (name, location, location_u8, user, reason, depends, date)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
ON CONFLICT(name)
DO UPDATE SET
  name = excluded.name,
  location = excluded.location,
  location_u8 = excluded.location_u8,
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
            module.location_u8,
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
        let module = module.as_ref();
        sqlx::query!("DELETE FROM modules WHERE name = ?1", module)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn get_module<S: AsRef<str>>(&self, name: S) -> Result<Option<StoreModule>> {
        let name = name.as_ref();

        // Fetch the row as an anonymous struct
        let row = sqlx::query!(
            r#"
SELECT name, location, location_u8, user, reason, depends, date as "date: chrono::DateTime<chrono::Utc>"
FROM modules
WHERE name = ?1
            "#,
            name
        )
        .fetch_optional(&self.pool)
        .await?;

        // Deserialize the depends JSON string
        match row {
            Some(module) => {
                let depends = match module.depends {
                    Some(json_str) if json_str == "[]" => None,
                    Some(json_str) => serde_json::from_str(&json_str)?,
                    None => None,
                };
                Ok(Some(
                    StoreModuleBuilder::default()
                        .with_name(module.name)
                        .with_location(module.location)
                        .with_location_u8(module.location_u8)
                        .with_user(module.user)
                        .with_reason(module.reason)
                        .with_depends(depends)
                        .with_date(module.date)
                        .build()?,
                ))
            }
            None => Ok(None),
        }
    }

    async fn get_all_modules(&self) -> Result<Vec<StoreModule>> {
        let rows = sqlx::query!(
            r#"
SELECT name, location, location_u8, user, reason, depends, date as "date: chrono::DateTime<chrono::Utc>"
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
                    location_u8: row.location_u8,
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

    async fn get_file<P: AsRef<Path>>(&self, filename: P) -> Result<Option<StoreFile>> {
        let filename_u8 = os_str_to_bytes(filename.as_ref());

        let result = sqlx::query_as!(StoreFile,
        r#"
SELECT files.source, files.source_u8, files.source_checksum, files.target, files.target_u8, files.target_checksum, files.operation, files.user, files.date as "date: chrono::DateTime<chrono::Utc>", modules.name AS module
FROM files
INNER JOIN modules ON files.module_id = modules.id
WHERE files.target_u8 = ?1
        "#, filename_u8).fetch_optional(&self.pool).await?;

        Ok(result)
    }

    async fn add_file(&self, file: StoreFile) -> Result<()> {
        // Retrieve the ID of the module
        let module_id = sqlx::query!("SELECT id FROM modules WHERE name = ?1", file.module)
            .fetch_one(&self.pool)
            .await?;

        sqlx::query!(
            r#"
INSERT INTO files (module_id, source, source_u8, source_checksum, target, target_u8, target_checksum, operation, user, date)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
ON CONFLICT(target)
DO UPDATE SET
  module_id = excluded.module_id,
  source = excluded.source,
  source_u8 = excluded.source_u8,
  source_checksum = excluded.source_checksum,
  target_checksum = excluded.target_checksum,
  operation = excluded.operation,
  user = excluded.user,
  date = excluded.date
            "#,
            module_id.id,
            file.source,
            file.source_u8,
            file.source_checksum,
            file.target,
            file.target_u8,
            file.target_checksum,
            file.operation,
            file.user,
            file.date
        ).execute(&self.pool).await?;

        Ok(())
    }

    async fn remove_file<P: AsRef<Path>>(&self, file: P) -> Result<()> {
        let file = os_str_to_bytes(file.as_ref());
        sqlx::query!("DELETE FROM files WHERE target_u8 = ?1", file)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn get_all_files<S: AsRef<str>>(&self, module: S) -> Result<Vec<StoreFile>> {
        let module = module.as_ref().to_owned();

        let result = sqlx::query_as!(StoreFile,
    r#"
SELECT files.source, files.source_u8, files.source_checksum, files.target, files.target_u8, files.target_checksum, files.operation, files.user, files.date as "date: chrono::DateTime<chrono::Utc>", modules.name AS module
FROM files
INNER JOIN modules ON files.module_id = modules.id
WHERE modules.name = ?1
    "#, module).fetch_all(&self.pool).await?;

        Ok(result)
    }

    async fn check_file_exists<P: AsRef<Path>>(&self, path: P) -> Result<bool> {
        let path_u8 = os_str_to_bytes(path.as_ref());

        debug!(
            "Looking for {} in {}",
            path.as_ref().display(),
            self.path.display()
        );

        let result = sqlx::query!("SELECT target FROM files WHERE target_u8 = ?1", path_u8)
            .fetch_optional(&self.pool)
            .await?;
        match result {
            Some(_) => {
                debug!(
                    "Found {} in {}",
                    path.as_ref().display(),
                    self.path.display()
                );
                Ok(true)
            }
            None => {
                debug!(
                    "Could not find {} in {}",
                    path.as_ref().display(),
                    self.path.display()
                );
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
        let filename_u8 = os_str_to_bytes(filename.as_ref());

        // Retrieve the checksum for the source file
        let file = sqlx::query!(
            "SELECT source_u8, source_checksum FROM files WHERE target_u8 = ?1",
            filename_u8
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(
            file.map_or(StoreSourceFileChecksum::new::<PathBuf>(None, None), |f| {
                StoreSourceFileChecksum::new(
                    f.source_u8.map(|x| PathBuf::from(bytes_to_os_str(x))),
                    f.source_checksum,
                )
            }),
        )
    }

    async fn get_target_checksum<P: AsRef<Path>>(
        &self,
        filename: P,
    ) -> Result<StoreTargetFileChecksum> {
        let filename_u8 = os_str_to_bytes(filename.as_ref());

        // Retrieve the checksum for the source file
        let file = sqlx::query_as!(
            StoreTargetFileChecksum,
            "SELECT target, target_checksum FROM files WHERE target_u8 = ?1",
            filename_u8
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(
            file.map_or(StoreTargetFileChecksum::new(filename.as_ref(), None), |f| {
                StoreTargetFileChecksum::new(f.target, f.target_checksum)
            }),
        )
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
        let module_id = match sqlx::query!("SELECT id FROM modules WHERE name = ?1", module)
            .fetch_optional(&self.pool)
            .await?
        {
            Some(id) => id,
            None => {
                warn!("Module {} not found in store", module);
                return Ok(vec![]);
            }
        };

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

    // --
    // * Tasks

    async fn get_task_uuids<S: AsRef<str>>(&self, module: S) -> Result<Vec<Uuid>> {
        let module = module.as_ref();
        let module_id = sqlx::query!("SELECT id FROM modules WHERE name = ?1", module)
            .fetch_one(&self.pool)
            .await?
            .id;

        let uuids = sqlx::query!(
            "SELECT uuid as \"uuid: Uuid\" FROM tasks WHERE module_id = ?1",
            module_id
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(uuids.into_iter().map(|r| r.uuid).collect::<Vec<Uuid>>())
    }

    async fn add_task(&self, data: PhaseTask) -> Result<()> {
        let module_id = sqlx::query!("SELECT id FROM modules WHERE name = ?1", data.module_name)
            .fetch_one(&self.pool)
            .await?
            .id;
        let uuid = data.calculate_uuid().await?;
        let data = serde_json::to_string(&data)?;

        sqlx::query!(
            r#"
INSERT INTO tasks (module_id, uuid, data)
VALUES (?1, ?2, ?3)
ON CONFLICT(uuid)
DO UPDATE SET data=excluded.data;
            "#,
            module_id,
            uuid,
            data
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_tasks<S: AsRef<str>>(&self, module: S) -> Result<Vec<PhaseTask>> {
        let module = module.as_ref();
        let module_id = sqlx::query!("SELECT id FROM modules WHERE name = ?1", module)
            .fetch_one(&self.pool)
            .await?
            .id;

        let tasks = sqlx::query!("SELECT data FROM tasks WHERE module_id = ?1", module_id)
            .fetch_all(&self.pool)
            .await?;

        let mut result = Vec::with_capacity(tasks.len());
        for task in tasks.into_iter() {
            result.push(serde_json::from_str(task.data.as_str())?);
        }
        Ok(result)
    }

    async fn get_task(&self, uuid: Uuid) -> Result<Option<PhaseTask>> {
        let task = sqlx::query!("SELECT data FROM tasks WHERE uuid = ?1", uuid)
            .fetch_optional(&self.pool)
            .await?;

        if let Some(task) = task {
            Ok(Some(serde_json::from_str(task.data.as_str())?))
        } else {
            Ok(None)
        }
    }

    async fn remove_task(&self, uuid: Uuid) -> Result<()> {
        sqlx::query!("DELETE FROM tasks WHERE uuid = ?1", uuid)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // --
    // * Messages

    async fn cache_message<S: AsRef<str>>(
        &self,
        command: S,
        message: CommandMessage,
    ) -> Result<()> {
        let module = &message.module_name;
        let command = command.as_ref();

        // Retrieve the ID of the module
        let module_id = sqlx::query!("SELECT id FROM modules WHERE name = ?1", module)
            .fetch_one(&self.pool)
            .await?;

        let message = serde_json::to_string(&message)?;

        sqlx::query!(
            r#"
INSERT INTO message_cache (module_id, command, data)
VALUES (?1, ?2, ?3)
            "#,
            module_id.id,
            command,
            message,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_all_cached_messages<S: AsRef<str>>(
        &self,
        module: S,
        command: S,
    ) -> Result<Vec<CommandMessage>> {
        let module = module.as_ref();
        let command = command.as_ref();

        // Retrieve the ID of the module
        let module_id = sqlx::query!("SELECT id FROM modules WHERE name = ?1", module)
            .fetch_one(&self.pool)
            .await?;

        let rows = sqlx::query!(
            "SELECT data FROM message_cache WHERE module_id = ?1 AND command = ?2",
            module_id.id,
            command
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| serde_json::from_str(r.data.as_str()))
            .collect::<Result<Vec<_>, _>>()?)
    }

    async fn remove_all_cached_messages<S: AsRef<str>>(&self, module: S, command: S) -> Result<()> {
        let module = module.as_ref();
        let command = command.as_ref();

        // Retrieve the ID of the module
        let module_id = sqlx::query!("SELECT id FROM modules WHERE name = ?1", module)
            .fetch_one(&self.pool)
            .await?;

        sqlx::query!(
            "DELETE FROM message_cache WHERE module_id = ?1 AND command = ?2",
            module_id.id,
            command,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

// -------------------------------------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{
        store::{sqlite_files::StoreFileBuilder, sqlite_modules::StoreModuleBuilder},
        tests::pm_setup,
    };
    use color_eyre::eyre::eyre;
    use std::ffi::OsString;
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
        let pool = init_sqlite_store(&config, pm).await?;

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

        pool.add_module(&test_module).await?;

        for i in 0..5 {
            let local_time = chrono::offset::Utc::now();
            let test_file = StoreFileBuilder::default()
                .with_module("test")
                .with_source(match op_tye {
                    "link" => Some(format!("/dotfiles/foo{}.txt", i)),
                    "copy" => Some(format!("/dotfiles/foo{}.txt", i)),
                    "create" => None,
                    _ => {
                        return Err(eyre!(
                            "Invalid 'which' parameter. Must be either 'link', 'copy' or 'create'."
                        ));
                    }
                })
                .with_source_u8(match op_tye {
                    "link" => Some(os_str_to_bytes(format!("/dotfiles/foo{}.txt", i))),
                    "copy" => Some(os_str_to_bytes(format!("/dotfiles/foo{}.txt", i))),
                    "create" => None,
                    _ => {
                        return Err(eyre!(
                            "Invalid 'which' parameter. Must be either 'link', 'copy' or 'create'."
                        ));
                    }
                })
                .with_source_checksum(match op_tye {
                    "link" => Some(format!("source_checksum{}", i)),
                    "copy" => Some(format!("source_checksum{}", i)),
                    "create" => None,
                    _ => {
                        return Err(eyre!(
                            "Invalid 'which' parameter. Must be either 'link', 'copy' or 'create'."
                        ));
                    }
                })
                .with_target(format!("/home/foo{}.txt", i))
                .with_target_u8(os_str_to_bytes(format!("/home/foo{}.txt", i)))
                .with_target_checksum(Some(format!("dest_checksum{}", i)))
                .with_operation(match op_tye {
                    "link" => "link".to_string(),
                    "copy" => "copy".to_string(),
                    "create" => "create".to_string(),
                    _ => {
                        return Err(eyre!(
                            "Invalid 'which' parameter. Must be either 'link', 'copy' or 'create'."
                        ));
                    }
                })
                .with_user(Some("user".to_string()))
                .with_date(local_time)
                .build()?;

            pool.add_file(test_file).await?;
        }

        Ok(pool)
    }
}
