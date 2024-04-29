use std::env;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use deadpool_sqlite::rusqlite::{params, OptionalExtension};
use deadpool_sqlite::{Config, Runtime};
use thiserror::Error;
use tokio::fs;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::process::Command;

use crate::common;
use crate::sudo;

// System cache /var/lib/dotdeploy
// User cache in $HOME
//
// variable controlling system cache usage
//
// init system cache
// DONE init user cache
// read system cache
// read user cache
// DONE get checksum
// add/update checksum
// DONE add/update file
// remove file

//
// Errors

#[derive(Error, Debug)]
pub enum SQLiteError {
    #[error("Failed to create pool")]
    CreatePoolFailed(#[from] deadpool_sqlite::CreatePoolError),
    #[error("Failed to execute SQL statement")]
    QueryError(#[from] deadpool_sqlite::rusqlite::Error),
    #[error("Failed to interact with connetion pool")]
    ConnectionInteractError(#[from] deadpool_sqlite::InteractError),
    #[error("Failed to get connection")]
    GetConnectionError(#[from] deadpool_sqlite::PoolError),
    #[error("Query returned invalid result")]
    InvalidQueryResult,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

// deadpool_sqlite::InteractError does not implement Sync but `(dyn Any + Send + 'static)` and is
// thus not directly compatible with anyhow. this function below should be called on all functions
// defined here if they are used in an anyhow context.
impl SQLiteError {
    /// Converts `SQLiteError` to `anyhow::Error`, handling non-Sync errors.
    pub fn into_anyhow(self) -> anyhow::Error {
        match self {
            SQLiteError::ConnectionInteractError(e) => {
                anyhow!("Connection interaction failed: {:?}", e)
            }
            _ => anyhow!("{:?}", self),
        }
    }
}

//
// Structs

/// Representation of the store database
#[derive(Clone, Debug)]
pub(crate) struct Store {
    /// SQLite connection pool
    pub(crate) pool: Option<deadpool_sqlite::Pool>,
    /// Store location
    pub(crate) path: PathBuf,
    /// User or system store
    system: bool,
}

/// Representation of a store module entry (row)
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StoreModule {
    pub(crate) name: String,
    pub(crate) location: String,
    pub(crate) user: Option<String>,
    pub(crate) reason: String,
    pub(crate) depends: Option<String>,
    pub(crate) date: chrono::DateTime<chrono::Local>,
}

/// Representation of a store file entry (row)
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StoreFile {
    pub(crate) module: String,
    pub(crate) source: Option<String>,
    pub(crate) source_checksum: Option<String>,
    pub(crate) destination: String,
    pub(crate) destination_checksum: Option<String>,
    /// Must be either 'link', 'copy' or 'create'.
    pub(crate) operation: String,
    pub(crate) user: Option<String>,
    pub(crate) date: chrono::DateTime<chrono::Local>,
}

/// Representation of a store backup entry (row)
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StoreBackup {
    /// Absolute file path
    pub(crate) path: String,
    /// Link or file
    pub(crate) file_type: String,
    /// Binary file
    pub(crate) content: Option<Vec<u8>>,
    /// Absolute file path to source
    pub(crate) link_source: Option<String>,
    /// User and group as string (UID:GID)
    pub(crate) owner: String,
    /// File permission
    pub(crate) permissions: Option<u32>,
    /// Sha256 checksum
    pub(crate) checksum: Option<String>,
    /// Date
    pub(crate) date: chrono::DateTime<chrono::Local>,
}

// Run maintanence and try to close the connection gracefully, thus the temporary `-wal` and `-shm`
// are cleaned up.
pub(crate) fn close_connection<P: AsRef<Path>>(path: P) -> Result<()> {
    while path
        .as_ref()
        .parent()
        .map_or_else(|| false, |p| p.join("store.sqlite-shm").exists())
    {
        while path
            .as_ref()
            .parent()
            .map_or_else(|| false, |p| p.join("store.sqlite-wal").exists())
        {
            let conn = deadpool_sqlite::rusqlite::Connection::open(&path)?;
            conn.pragma_update(
                Some(deadpool_sqlite::rusqlite::DatabaseName::Main),
                "journal_mode",
                "WAL",
            )
            .context("Failed to run PRAGMA journal_mode=WAL")?;
            conn.pragma_update(
                Some(deadpool_sqlite::rusqlite::DatabaseName::Main),
                "synchronous",
                "NORMAL",
            )
            .context("Failed to run PRAGMA synchronous=NORMAL")?;

            conn.execute_batch("VACUUM;;")
                .context("Failed to run VACUUM;")?;

            drop(conn);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    Ok(())
}

fn prepare_connection(
    connection: &mut deadpool_sqlite::rusqlite::Connection,
) -> Result<(), SQLiteError> {
    connection
        .pragma_update(
            Some(deadpool_sqlite::rusqlite::DatabaseName::Main),
            "journal_mode",
            "WAL",
        )
        .context("Failed to run PRAGMA journal_mode=WAL")?;
    connection
        .pragma_update(
            Some(deadpool_sqlite::rusqlite::DatabaseName::Main),
            "synchronous",
            "NORMAL",
        )
        .context("Failed to run PRAGMA synchronous=NORMAL")?;
    Ok(())
}

impl Store {
    // Initialization

    /// Creates a new Store configuration but does not initialize the connection pool yet.
    fn new(path: PathBuf, system: bool) -> Self {
        Store {
            pool: None,
            path,
            system,
        }
    }

    async fn create_dir(&self) -> Result<()> {
        // Check if system store should be created.
        if self.system {
            match self.path.try_exists() {
                Ok(false) => {
                    debug!(
                        "Store directory '{}' does not exist, creating.",
                        &self.path.display()
                    );

                    sudo::sudo_exec(
                        "mkdir",
                        &["-p", common::path_to_string(&self.path)?.as_str()],
                        Some("Create directory for system store DB"),
                    )
                    .await
                    .with_context(|| format!("Failed to create directory {:?}", &self.path))?;

                    // Adjust permissions so everybody can write to the directory. This makes
                    // communication with the store easier and does not require elevated permissions to
                    // access it.
                    sudo::sudo_exec(
                        "chmod",
                        &["777", common::path_to_string(&self.path)?.as_str()],
                        Some("Adjusting permissions of system store DB directory"),
                    )
                    .await
                    .with_context(|| {
                        format!("Failed to change permissions of directory {:?}", &self.path)
                    })?;
                }
                Ok(true) => {
                    debug!(
                        "Store directory '{}' exists already, continuing.",
                        &self.path.display()
                    );
                }
                Err(e) => bail!("{}", e),
            }
        } else {
            match self.path.try_exists() {
                Ok(false) => {
                    debug!(
                        "Store directory '{}' does not exist, creating.",
                        &self.path.display()
                    );
                    Command::new("mkdir")
                        .arg("-p")
                        .arg(&self.path)
                        .status()
                        .await
                        .with_context(|| format!("Failed to create directory {:?}", &self.path))?;
                }
                Ok(true) => {
                    debug!(
                        "Store directory '{}' exists already, continuing.",
                        &self.path.display()
                    );
                }
                Err(e) => bail!("{}", e),
            }
        }
        Ok(())
    }

    /// Initiliazes a store database.
    ///
    /// The database will be initialized under `path` and named `store.sqlite`.
    ///
    /// # Returns
    /// Returns `Ok()` if successful.
    async fn init(&mut self) -> Result<(), SQLiteError> {
        // Check if database directory exists and if not, create it.
        self.create_dir().await.map_err(SQLiteError::Other)?;

        // Set full path
        self.path = self.path.join("store.sqlite");

        // Set database options
        let pool = Config::new(&self.path)
            .create_pool(Runtime::Tokio1)
            .with_context(|| {
                format!("Failed to create pool for store database {:?}", &self.path)
            })?;

        let conn = pool
            .get()
            .await
            .with_context(|| format!("Failed to connect to store database {:?}", &self.path))?;

        // Enable WAL mode and auto-vacuum before any tables are created
        conn.interact(move |conn| -> Result<(), SQLiteError> {
            prepare_connection(conn)?;
            Ok(())
        })
        .await??;

        // Create MODULES table
        conn.interact(|conn| -> Result<(), SQLiteError> {
            prepare_connection(conn)?;
            conn.execute(
                "CREATE TABLE IF NOT EXISTS modules (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               name TEXT NOT NULL UNIQUE,
               location TEXT NOT NULL,
               user TEXT,
               reason TEXT NOT NULL,
               depends TEXT,
               date TEXT NOT NULL
         );",
                [],
            )
            .context("Failed to create MODULES table")?;
            Ok(())
        })
        .await??;

        // Create FILES table
        conn.interact(|conn| -> Result<(), SQLiteError> {
            prepare_connection(conn)?;
            conn.execute(
                "CREATE TABLE IF NOT EXISTS files (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               module_id INTEGER,
               source TEXT,
               source_checksum TEXT,
               destination TEXT NOT NULL UNIQUE,
               destination_checksum TEXT,
               operation TEXT NOT NULL,
               user TEXT,
               date TEXT NOT NULL,
               FOREIGN KEY (module_id) REFERENCES modules(id)
               ON DELETE CASCADE ON UPDATE CASCADE
             );",
                [],
            )
            .context("Failed to create FILES table")?;
            Ok(())
        })
        .await??;

        // Create BACKUPS table
        conn.interact(|conn| -> Result<(), SQLiteError> {
            prepare_connection(conn)?;
            conn.execute(
                "CREATE TABLE IF NOT EXISTS backups (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               path TEXT NOT NULL UNIQUE,
               file_type TEXT NOT NULL,
               content BLOB,
               link_source TEXT,
               owner TEXT NOT NULL,
               permissions INTEGER,
               checksum TEXT,
               date TEXT NOT NULL
             );",
                [],
            )
            .context("Failed to create BACKUPS table")?;
            Ok(())
        })
        .await??;

        // Store pool
        self.pool = Some(pool);
        Ok(())
    }

    /// Close all connections to pool
    pub(crate) async fn close(&self) -> Result<(), SQLiteError> {
        if let Some(pool) = &self.pool {
            while pool.status().waiting != 0 {
                debug!("{} DB tasks waiting for completion", pool.status().waiting);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let conn = &self.get_con().await?;
            // Run optimize
            conn.interact(|conn| -> Result<(), SQLiteError> {
                prepare_connection(conn)?;
                conn.execute_batch("PRAGMA optimize")
                    .context("Failed to run PRAGMA optimize")?;

                Ok(())
            })
            .await??;

            pool.close();
            Ok(())
        } else {
            Err(anyhow!("Store database not initialized!").into())
        }
    }

    /// Return a connection to store pool
    pub(crate) async fn get_con(&self) -> Result<deadpool_sqlite::Object, SQLiteError> {
        if let Some(pool) = &self.pool {
            Ok(pool.get().await?)
        } else {
            Err(anyhow!("Store database not initialized!").into())
        }
    }
    // Module operations

    /// Add or update a module in the database.
    pub(crate) async fn add_module(&self, module: StoreModule) -> Result<(), SQLiteError> {
        let conn = &self.get_con().await?;
        // Attempt to insert the module, ignoring the operation if the module already exists.
        conn.interact(move |conn| -> Result<(), SQLiteError> {
            prepare_connection(conn)?;
            conn.execute(
                "INSERT INTO modules (name, location, user, reason, depends, date) VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT(name) DO NOTHING",
                params![module.name, module.location, module.user, module.reason, module.depends, module.date]
            )?;
            Ok(())
        })
        .await??;
        Ok(())
    }

    /// Remove a module in the database.
    pub(crate) async fn remove_module<S: AsRef<str>>(&self, module: S) -> Result<(), SQLiteError> {
        let module = module.as_ref().to_owned();
        let conn = &self.get_con().await?;
        // Attempt to insert the module, ignoring the operation if the module already exists.
        conn.interact(move |conn| -> Result<(), SQLiteError> {
            prepare_connection(conn)?;
            conn.execute("DELETE FROM modules WHERE name = $1", params![module])?;
            Ok(())
        })
        .await??;

        Ok(())
    }

    /// Retrieve a single module from store.
    pub(crate) async fn get_module<S: AsRef<str>>(
        &self,
        name: S,
    ) -> Result<StoreModule, SQLiteError> {
        let name = name.as_ref().to_owned();
        let conn = &self.get_con().await?;

        conn.interact(move |conn| -> Result<StoreModule, SQLiteError> {
            prepare_connection(conn)?;
            let mut stmt = conn.prepare(
                "SELECT name, location, user, reason, depends, date
                 FROM modules
                 WHERE name = $1",
            )?;

            Ok(stmt.query_row(params![name], |row| {
                Ok(StoreModule {
                    name: row.get(0)?,
                    location: row.get(1)?,
                    user: row.get(2)?,
                    reason: row.get(3)?,
                    depends: row.get(4)?,
                    date: row.get(5)?,
                })
            })?)
        })
        .await?
    }

    /// Retrieve all modules
    pub(crate) async fn get_all_modules(&self) -> Result<Vec<StoreModule>, SQLiteError> {
        let conn = &self.get_con().await?;

        conn.interact(move |conn| -> Result<Vec<StoreModule>, SQLiteError> {
            prepare_connection(conn)?;
            let mut stmt = conn.prepare(
                "SELECT name, location, user, reason, depends, date
                 FROM modules",
            )?;

            let rows: Vec<Result<StoreModule, deadpool_sqlite::rusqlite::Error>> = stmt
                .query_map(params![], |row| {
                    Ok(StoreModule {
                        name: row.get(0)?,
                        location: row.get(1)?,
                        user: row.get(2)?,
                        reason: row.get(3)?,
                        depends: row.get(4)?,
                        date: row.get(5)?,
                    })
                })?
                .collect();

            let mut modules = Vec::with_capacity(rows.len());
            for row in rows {
                match row {
                    Ok(row) => modules.push(row),
                    Err(e) => eprintln!("Error processing row: {:?}", e), // Log or handle the error
                }
            }
            Ok(modules)
        })
        .await?
    }

    // File operations

    /// Retrieve a single file from store.
    pub(crate) async fn get_file<P: AsRef<Path>>(
        &self,
        filename: P,
    ) -> Result<StoreFile, SQLiteError> {
        // Safely handle the possibility that the path cannot be converted to a &str
        let filename_str = common::path_to_string(filename)?;

        let conn = &self.get_con().await?;

        conn.interact(move |conn| -> Result<StoreFile, SQLiteError> {
            prepare_connection(conn)?;
            let mut stmt = conn.prepare(
                "SELECT files.id, files.source, files.source_checksum, files.destination, files.destination_checksum, files.operation, files.user, files.date, modules.name AS module
                 FROM files
                 INNER JOIN modules ON files.module_id = modules.id
                 WHERE files.destination = $1")?;

            Ok(stmt.query_row(params![filename_str], |row| {
                Ok(StoreFile {
                    module: row.get(8)?,
                    source: row.get(1)?,
                    source_checksum: row.get(2)?,
                    destination: row.get(3)?,
                    destination_checksum: row.get(4)?,
                    operation: row.get(5)?,
                    user: row.get(6)?,
                    date: row.get(7)?,
                })
            })?)
        }).await?
    }

    /// Add or update a single file in the database.
    pub(crate) async fn add_file(&self, file: StoreFile) -> Result<(), SQLiteError> {
        let conn = &self.get_con().await?;

        // Retrieve the ID of the module
        let module_id = conn
            .interact(move |conn| -> Result<i64, SQLiteError> {
                prepare_connection(conn)?;
                Ok(conn.query_row(
                    "SELECT id FROM modules WHERE name = $1",
                    params![file.module],
                    |row| row.get(0),
                )?)
            })
            .await??;

        conn.interact(move |conn| -> Result<(), SQLiteError> {
            prepare_connection(conn)?;
            let mut stmt = conn.prepare(
                "INSERT INTO files (module_id, source, source_checksum, destination, destination_checksum, operation, user, date)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                 ON CONFLICT(destination)
                 DO UPDATE SET
                   module_id = excluded.module_id,
                   source = excluded.source,
                   source_checksum = excluded.source_checksum,
                   destination_checksum = excluded.destination_checksum,
                   operation = excluded.operation,
                   user = excluded.user,
                   date = excluded.date")?;

            stmt.execute(params![
                module_id,
                &file.source,
                &file.source_checksum,
                &file.destination,
                &file.destination_checksum,
                &file.operation,
                &file.user,
                &file.date]
            )?;

            Ok(())
        }).await??;

        Ok(())
    }

    /// Remove a single file in the database.
    pub(crate) async fn remove_file<S: AsRef<str>>(&self, file: S) -> Result<(), SQLiteError> {
        let file = file.as_ref().to_owned();
        let conn = &self.get_con().await?;
        // Attempt to remove the file, ignoring the operation if the file already exists.
        conn.interact(move |conn| -> Result<(), SQLiteError> {
            prepare_connection(conn)?;
            conn.execute("DELETE FROM files WHERE destination = $1", params![file])?;
            Ok(())
        })
        .await??;

        Ok(())
    }

    /// Retrieve all files from a module
    pub(crate) async fn get_all_files<S: AsRef<str>>(
        &self,
        module: S,
    ) -> Result<Vec<StoreFile>, SQLiteError> {
        let module = module.as_ref().to_owned();
        let conn = &self.get_con().await?;

        conn.interact(move |conn| -> Result<Vec<StoreFile>, SQLiteError> {
            prepare_connection(conn)?;
            let mut stmt = conn.prepare(
                "SELECT files.id, files.source, files.source_checksum, files.destination, files.destination_checksum, files.operation, files.user, files.date, modules.name AS module
                 FROM files
                 INNER JOIN modules ON files.module_id = modules.id
                 WHERE modules.name = $1"
            )?;

            let rows: Vec<Result<StoreFile, deadpool_sqlite::rusqlite::Error>> = stmt.query_map(params![module], |row| {
                Ok(StoreFile {
                    module: row.get(8)?,
                    source: row.get(1)?,
                    source_checksum: row.get(2)?,
                    destination: row.get(3)?,
                    destination_checksum: row.get(4)?,
                    operation: row.get(5)?,
                    user: row.get(6)?,
                    date: row.get(7)?,
                })
            })?.collect();

            let mut files = Vec::with_capacity(rows.len());
            for row in rows {
                match row {
                    Ok(row) => files.push(row),
                    Err(e) => eprintln!("Error processing row: {:?}", e), // Log or handle the error
                }
            }
            Ok(files)
        }).await?
    }

    /// Check if a file exists in the store database.
    pub(crate) async fn check_file_exists<P: AsRef<Path>>(
        &self,
        path: P,
    ) -> Result<bool, SQLiteError> {
        let path_str = common::path_to_string(path)?;
        let store_path = self.path.clone();

        debug!("Looking for {} in {}", &path_str, &self.path.display());

        let conn = &self.get_con().await?;

        let result = conn
            .interact(move |conn| -> Result<bool, SQLiteError> {
                prepare_connection(conn)?;
                let mut stmt =
                    conn.prepare("SELECT destination FROM files where destination = $1")?;

                match stmt.query_row(params![path_str], |row| row.get::<_, String>(0)) {
                    Ok(_) => {
                        debug!("Found {} in {}", &path_str, &store_path.display());
                        Ok(true)
                    }
                    Err(e) if e == deadpool_sqlite::rusqlite::Error::QueryReturnedNoRows => {
                        debug!("Could not find {} in {}", &path_str, &store_path.display());
                        Ok(false)
                    }
                    Err(e) => Err(SQLiteError::QueryError(e)),
                }
            })
            .await??;

        Ok(result)
    }

    // Checksums

    /// Retrieve checksum of either source or destination file from store database.
    async fn get_file_checksum<S: AsRef<str>>(
        &self,
        filename: S,
        which: &str,
    ) -> Result<Option<(String, String)>, SQLiteError> {
        let filename = filename.as_ref().to_owned();
        let which = which.to_owned();
        let conn = &self.get_con().await?;

        let query = conn
            .interact(
                move |conn| -> Result<Option<(Option<String>, Option<String>)>, SQLiteError> {
                    prepare_connection(conn)?;
                    let mut stmt = conn.prepare(match which.as_str() {
                        "source" => "SELECT source, source_checksum FROM files WHERE destination = $1",
                        "destination" => {
                            "SELECT destination, destination_checksum FROM files WHERE destination = $1"
                        }
                        _ => {
                            return Err(anyhow!(
                                "Invalid 'which' parameter. Must be either 'source' or 'destination'."
                            )
                                       .into())
                        }
                    })?;

                    Ok(stmt
                       .query_row(params![filename], |row| Ok((row.get(0)?, row.get(1)?)))
                       .optional()?)
                },
            )
            .await??;

        // Check if both parts of the tuple are None and return None for the whole result in
        // that case. Otherwise, map the tuple of Options into a tuple of Strings, providing
        // default values as needed.
        let result = match query {
            Some((Some(file), Some(checksum))) => Some((file, checksum)),
            _ => None,
        };

        Ok(result)
    }

    /// Retrieve checksum of source file from store database.
    pub(crate) async fn get_source_checksum<P: AsRef<Path>>(
        &self,
        filename: P,
    ) -> Result<Option<(String, String)>, SQLiteError> {
        // Safely handle the possibility that the path cannot be converted to a &str
        let filename_str = common::path_to_string(filename)?;
        self.get_file_checksum(filename_str, "source").await
    }

    /// Retrieve checksum of destination file from store database.
    pub(crate) async fn get_destination_checksum<P: AsRef<Path>>(
        &self,
        filename: P,
    ) -> Result<Option<(String, String)>, SQLiteError> {
        // Safely handle the possibility that the path cannot be converted to a &str
        let filename_str = common::path_to_string(filename)?;
        self.get_file_checksum(filename_str, "destination").await
    }

    /// Retrieve all checksums of either source or destination files from store database.
    async fn get_all_checksums(
        &self,
        which: &str,
    ) -> Result<Vec<(Option<String>, Option<String>)>, SQLiteError> {
        let which = which.to_owned();
        let conn = &self.get_con().await?;

        let query = conn
            .interact(
                move |conn| -> Result<Vec<(Option<String>, Option<String>)>, SQLiteError> {
                    prepare_connection(conn)?;
                    let mut stmt = conn.prepare(match which.as_str() {
                        "source" => "SELECT source, source_checksum FROM files",
                        "destination" => "SELECT destination, destination_checksum FROM files",
                        _ => {
                            return Err(anyhow!(
                            "Invalid 'which' parameter. Must be either 'source' or 'destination'."
                        )
                            .into())
                        }
                    })?;

                    let rows: Vec<
                        Result<(Option<String>, Option<String>), deadpool_sqlite::rusqlite::Error>,
                    > = stmt
                        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                        .collect();

                    let mut checksums = Vec::with_capacity(rows.len());
                    for row in rows {
                        match row {
                            Ok(row) => checksums.push(row),
                            Err(e) => eprintln!("Error processing row: {:?}", e), // Log or handle the error
                        }
                    }

                    Ok(checksums)
                },
            )
            .await??;

        // Filter out entries where both the file and checksum are None.
        let result: Vec<(Option<String>, Option<String>)> = query
            .into_iter()
            .filter(|(file, checksum)| file.is_some() || checksum.is_some())
            .collect();

        Ok(result)
    }

    /// Retrieve all source checksums from store database.
    pub(crate) async fn get_all_src_checksums(
        &self,
    ) -> Result<Vec<(Option<String>, Option<String>)>, SQLiteError> {
        self.get_all_checksums("source").await
    }

    /// Retrieve all destination checksums from store database.
    pub(crate) async fn get_all_dest_checksums(
        &self,
    ) -> Result<Vec<(Option<String>, Option<String>)>, SQLiteError> {
        self.get_all_checksums("destination").await
    }

    //
    // File backups

    /// Add the backup of a file to the store database.
    pub(crate) async fn add_backup<P: AsRef<Path>>(&self, file_path: P) -> Result<(), SQLiteError> {
        // Safely handle the possibility that the path cannot be converted to a &str
        let file_path_str = common::path_to_string(&file_path)?;

        // Collect metadata
        // We should always be able to read metadata but not if the file is in a directory which
        // denies access. Then, we need to copy the file first to an accessible location.
        let mut sql_stmt = String::new();

        let metadata = common::get_file_metadata(&file_path).await?;

        let b_file: StoreBackup = if metadata.is_symlink {
            let link_source = common::path_to_string(metadata.symlink_source.unwrap())?;

            sql_stmt.push_str(
                "INSERT INTO backups (path, file_type, content, link_source, owner, permissions, checksum, date) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                 ON CONFLICT(path) DO NOTHING"
            );
            let user_id = metadata
                .uid
                .ok_or_else(|| anyhow!("Could not get UID of {:?}", &file_path_str))
                .map_err(SQLiteError::Other)?;

            let group_id = metadata
                .gid
                .ok_or_else(|| anyhow!("Could not get GID of {:?}", &file_path_str))
                .map_err(SQLiteError::Other)?;

            StoreBackup {
                path: file_path_str,
                file_type: "link".to_string(),
                content: None,
                link_source: Some(link_source),
                owner: format!("{}:{}", user_id, group_id),
                permissions: None,
                checksum: None,
                date: chrono::offset::Local::now(),
            }
        } else {
            sql_stmt.push_str(
                "INSERT INTO backups (path, file_type, content, link_source, owner, permissions, checksum, date) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                 ON CONFLICT(path) DO NOTHING"
            );

            let user_id = metadata
                .uid
                .ok_or_else(|| anyhow!("Could not get UID of {:?}", &file_path_str))
                .map_err(SQLiteError::Other)?;

            let group_id = metadata
                .gid
                .ok_or_else(|| anyhow!("Could not get GID of {:?}", &file_path_str))
                .map_err(SQLiteError::Other)?;

            let permissions = metadata
                .permissions
                .ok_or_else(|| anyhow!("Could not get permissions of {:?}", &file_path_str))
                .map_err(SQLiteError::Other)?;

            let checksum = metadata
                .checksum
                .ok_or_else(|| anyhow!("Could not get checksum of {:?}", &file_path_str))
                .map_err(SQLiteError::Other)?;

            let content = match fs::read(&file_path).await {
                Ok(c) => c,
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    let temp_file =
                        tempfile::NamedTempFile::new().map_err(|e| SQLiteError::Other(e.into()))?;
                    let temp_path_str = common::path_to_string(&temp_file)?;

                    sudo::sudo_exec(
                        "cp",
                        &[
                            "--preserve",
                            "--no-dereference",
                            &common::path_to_string(&file_path)?,
                            &temp_path_str,
                        ],
                        Some(
                            format!(
                                "Create temporary copy of {:?} for backup creation",
                                &file_path.as_ref()
                            )
                            .as_str(),
                        ),
                    )
                    .await?;

                    // Ensure that we can read the file which we want to backup
                    common::set_file_metadata(
                        &temp_file,
                        common::FileMetadata {
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
                        .with_context(|| format!("Failed to read {:?}", &temp_file))?
                }
                Err(e) => {
                    Err(e).with_context(|| format!("Failed to read {:?}", &file_path.as_ref()))?
                }
            };

            StoreBackup {
                path: file_path_str,
                file_type: "regular".to_string(),
                content: Some(content),
                link_source: None,
                owner: format!("{}:{}", user_id, group_id),
                permissions: Some(permissions),
                checksum: Some(checksum),
                date: chrono::offset::Local::now(),
            }
        };

        let conn = &self.get_con().await?;

        conn.interact(move |conn| -> Result<(), SQLiteError> {
            prepare_connection(conn)?;
            let mut stmt = conn.prepare(&sql_stmt)?;

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

    /// Add the backup of a file to the store database.
    pub(crate) async fn remove_backup<P: AsRef<Path>>(
        &self,
        file_path: P,
    ) -> Result<(), SQLiteError> {
        let file_path_str = common::path_to_string(&file_path)?;

        let conn = &self.get_con().await?;
        conn.interact(move |conn| -> Result<(), SQLiteError> {
            prepare_connection(conn)?;
            conn.execute(
                "DELETE FROM backups WHERE path = $1",
                params![file_path_str],
            )?;
            Ok(())
        })
        .await??;

        Ok(())
    }

    /// Check if a backup of a file exists in the store database.
    pub(crate) async fn check_backup_exists<P: AsRef<Path>>(
        &self,
        path: P,
    ) -> Result<bool, SQLiteError> {
        let path_str = common::path_to_string(path)?;
        let store_path = self.path.clone();

        debug!(
            "Looking for backup of {} in {}",
            &path_str,
            &store_path.display()
        );

        let conn = &self.get_con().await?;

        let result = conn
            .interact(move |conn| -> Result<bool, SQLiteError> {
                prepare_connection(conn)?;
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

    /// Restore backup from store database.
    /// Returns true if the backup restored is valid.
    pub(crate) async fn restore_backup<P: AsRef<Path>>(
        &self,
        file_path: P,
        to: P,
    ) -> Result<(), SQLiteError> {
        // Safely handle the possibility that the path cannot be converted to a &str
        let file_path_str = common::path_to_string(&file_path)?;

        let conn = &self.get_con().await?;

        let result = conn
            .interact(move |conn| -> Result<StoreBackup, SQLiteError> {
                prepare_connection(conn)?;
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
            .await??;

        let owner: Vec<&str> = result.owner.split(':').collect();

        match result.file_type.as_str() {
            "link" => {
                match fs::symlink(result.link_source.as_ref().unwrap(), &to).await {
                    Ok(_) => (),
                    Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                        sudo::sudo_exec(
                            "ln",
                            &[
                                "-sf",
                                result.link_source.as_ref().unwrap(),
                                to.as_ref().to_str().unwrap(),
                            ],
                            None,
                        )
                        .await?;
                    }
                    Err(e) => Err(e).with_context(|| {
                        format!("Falied to restore backup of {:?}", &file_path.as_ref())
                    })?,
                }
                common::set_file_metadata(
                    file_path,
                    common::FileMetadata {
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
                        permissions: None,
                        is_symlink: true,
                        symlink_source: None,
                        checksum: None,
                    },
                )
                .await?;
            }
            "regular" => {
                let mut write_dest = PathBuf::new();
                let temp_file =
                    tempfile::NamedTempFile::new().map_err(|e| SQLiteError::Other(e.into()))?;
                let mut move_file = false;

                let file = match fs::File::create(&to).await {
                    Ok(f) => {
                        write_dest.push(&to);
                        f
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                        write_dest.push(&temp_file);
                        move_file = true;
                        fs::File::create(&temp_file)
                            .await
                            .map_err(|e| SQLiteError::Other(e.into()))?
                    }
                    Err(e) => Err(e).with_context(|| {
                        format!("Falied to restore backup of {:?}", &file_path.as_ref())
                    })?,
                };

                {
                    let mut writer = BufWriter::new(file);
                    writer
                        .write(&result.content.unwrap())
                        .await
                        .with_context(|| format!("Failed to write to file {:?}", &write_dest))?;
                    writer
                        .flush()
                        .await
                        .context("Failed to flush write buffer")?;
                }

                common::set_file_metadata(
                    &write_dest,
                    common::FileMetadata {
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
                        permissions: Some(result.permissions.unwrap()),
                        is_symlink: false,
                        symlink_source: None,
                        checksum: None,
                    },
                )
                .await?;

                if move_file {
                    sudo::sudo_exec(
                        "cp",
                        &[
                            "--preserve",
                            &write_dest.to_str().unwrap(),
                            to.as_ref().to_str().unwrap(),
                        ],
                        None,
                    )
                    .await?;
                }
            }
            _ => unreachable!(),
        }

        // Validate file permissions
        // let mut valid = false;
        // let metadata = to.metadata()?;
        // let user_id = metadata.uid();
        // let group_id = metadata.gid();
        // let permissions = metadata.mode();
        // let checksum = crate::phases::calculate_sha256_checksum(&to).await?;

        // if owner[0] != user_id || owner[1] != group_id
        Ok(())
    }
}

/// Initialize user store.
///
/// The database will be initialized in `path` or `XDG_DATA_HOME/dotdeploy` or, if this variable is not set,
/// in `HOME/.local/share/dotdeploy`.
///
/// # Returns
/// Returns `Ok(Store)` if successful.
pub(crate) async fn init_user_store(path: Option<PathBuf>) -> Result<Store, SQLiteError> {
    // Try to build the path from XDG_DATA_HOME, else from HOME
    let store_path: PathBuf = path.unwrap_or(if let Ok(xdg_dir) = env::var("XDG_DATA_HOME") {
        [xdg_dir.as_str(), "dotdeploy"].iter().collect()
    } else {
        [
            env::var("HOME")
                .map_err(|e| SQLiteError::Other(e.into()))?
                .as_str(),
            ".local",
            "share",
            "dotdeploy",
        ]
        .iter()
        .collect()
    });

    // Connect to database
    let mut store = Store::new(store_path.clone(), false);
    store
        .init()
        .await
        .map_err(|e| e.into_anyhow())
        .with_context(|| {
            format!(
                "Failed to initialize user store in {}",
                &store_path.display()
            )
        })?;
    Ok(store)
}

/// Initialize system store.
///
/// The database will be initialized in `/var/lib/dotdeploy`.
///
/// # Returns
/// Returns `Ok(Store)` if successful.
pub(crate) async fn init_system_store() -> Result<Store, SQLiteError> {
    // Set store path
    let store_path: PathBuf = PathBuf::from("/var/lib/dotdeploy");

    // Connect to database
    let mut store = Store::new(store_path.clone(), true);
    store
        .init()
        .await
        .map_err(|e| e.into_anyhow())
        .context("Failed to initialize system store in /var/lib/dotdeploy")?;

    fs::set_permissions(&store.path, std::fs::Permissions::from_mode(0o666))
        .await
        .map_err(|e| SQLiteError::Other(e.into()))?;
    Ok(store)
}

//
// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_init_user_store() -> Result<(), SQLiteError> {
        let temp_dir = tempdir().map_err(|e| SQLiteError::Other(e.into()))?;

        // Init store
        let user_store = init_user_store(Some(temp_dir.into_path())).await?;

        // Insert a module
        let test_module = StoreModule {
            name: "test".to_string(),
            location: "/testpath".to_string(),
            user: Some("user".to_string()),
            reason: "manual".to_string(),
            depends: None,
            date: chrono::offset::Local::now(),
        };

        user_store.add_module(test_module).await?;

        let conn = user_store.get_con().await?;
        let result = conn
            .interact(move |conn| -> Result<String, SQLiteError> {
                prepare_connection(conn)?;
                Ok(conn.query_row("SELECT name FROM modules WHERE id=1", [], |row| row.get(0))?)
            })
            .await??;

        assert_eq!(result, "test".to_string());
        Ok(())
    }

    #[tokio::test]
    async fn test_add_and_get_file() -> Result<()> {
        let temp_dir = tempdir()?;
        // Init store
        let user_store = init_user_store(Some(temp_dir.into_path()))
            .await
            .map_err(|e| e.into_anyhow())?;

        // Insert a module
        let test_module = StoreModule {
            name: "test".to_string(),
            location: "/testpath".to_string(),
            user: Some("user".to_string()),
            reason: "manual".to_string(),
            depends: None,
            date: chrono::offset::Local::now(),
        };

        user_store
            .add_module(test_module)
            .await
            .map_err(|e| e.into_anyhow())?;

        // Test input
        let local_time = chrono::offset::Local::now();
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

        user_store
            .add_file(test_file.clone())
            .await
            .map_err(|e| e.into_anyhow())?;

        let result = user_store
            .get_file("/home/foo.txt")
            .await
            .map_err(|e| e.into_anyhow())?;

        assert_eq!(test_file, result);

        // Missing file
        let e = user_store.get_file("/doesNotExist.txt").await;
        assert!(e.is_err());

        Ok(())
    }

    async fn store_setup_helper(op_tye: &str) -> Result<Store> {
        let temp_dir = tempdir()?;

        // Initialize the user store, which sets up the database and tables
        let pool = init_user_store(Some(temp_dir.into_path()))
            .await
            .map_err(|e| e.into_anyhow())?;

        // Insert a module
        let test_module = StoreModule {
            name: "test".to_string(),
            location: "/testpath".to_string(),
            user: Some("user".to_string()),
            reason: "manual".to_string(),
            depends: None,
            date: chrono::offset::Local::now(),
        };

        pool.add_module(test_module)
            .await
            .map_err(|e| e.into_anyhow())?;

        for i in 0..5 {
            let local_time = chrono::offset::Local::now();
            let test_file = StoreFile {
                module: "test".to_string(),
                source: match op_tye {
                    "link" => Some(format!("/dotfiles/foo{}.txt", i)),
                    "copy" => Some(format!("/dotfiles/foo{}.txt", i)),
                    "create" => None,
                    _ => bail!(
                        "Invalid 'which' parameter. Must be either 'link', 'copy' or 'create'."
                    ),
                },
                source_checksum: match op_tye {
                    "link" => Some(format!("source_checksum{}", i)),
                    "copy" => Some(format!("source_checksum{}", i)),
                    "create" => None,
                    _ => bail!(
                        "Invalid 'which' parameter. Must be either 'link', 'copy' or 'create'."
                    ),
                },
                destination: format!("/home/foo{}.txt", i),
                destination_checksum: Some(format!("dest_checksum{}", i)),
                operation: match op_tye {
                    "link" => "link".to_string(),
                    "copy" => "copy".to_string(),
                    "create" => "create".to_string(),
                    _ => bail!(
                        "Invalid 'which' parameter. Must be either 'link', 'copy' or 'create'."
                    ),
                },
                user: Some("user".to_string()),
                date: local_time,
            };

            pool.add_file(test_file)
                .await
                .map_err(|e| e.into_anyhow())?;
        }

        Ok(pool)
    }

    #[tokio::test]
    async fn test_get_all_files() -> Result<()> {
        let store = store_setup_helper("link").await?;
        let result = store
            .get_all_files("test")
            .await
            .map_err(|e| e.into_anyhow())?;

        assert_eq!(result.len(), 5);
        assert_eq!(result[2].module, "test");
        assert_eq!(result[2].destination, "/home/foo2.txt");

        // Missing module
        let e = store
            .get_all_files("Foobar")
            .await
            .map_err(|e| e.into_anyhow())?;
        assert!(e.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_get_checksums() -> Result<()> {
        let store = store_setup_helper("link").await?;

        // Get single checksum
        let result = store
            .get_source_checksum("/home/foo2.txt")
            .await
            .map_err(|e| e.into_anyhow())?;
        assert_eq!(
            result,
            Some((
                "/dotfiles/foo2.txt".to_string(),
                "source_checksum2".to_string()
            ))
        );
        let result = store
            .get_destination_checksum("/home/foo3.txt")
            .await
            .map_err(|e| e.into_anyhow())?;
        assert_eq!(
            result,
            Some(("/home/foo3.txt".to_string(), "dest_checksum3".to_string()))
        );
        let result = store
            .get_destination_checksum("/does/not/exist.txt")
            .await
            .map_err(|e| e.into_anyhow())?;
        assert_eq!(result, None);

        // Source file and source checksum missing
        let store = store_setup_helper("create").await?;
        let result = store
            .get_source_checksum("/home/foo2.txt")
            .await
            .map_err(|e| e.into_anyhow())?;
        assert_eq!(result, None);

        // All checksums
        let store = store_setup_helper("create").await?;
        let result = store
            .get_all_src_checksums()
            .await
            .map_err(|e| e.into_anyhow())?;
        assert_eq!(result.len(), 0);

        let store = store_setup_helper("create").await?;
        let result = store
            .get_all_dest_checksums()
            .await
            .map_err(|e| e.into_anyhow())?;
        assert_eq!(result.len(), 5);

        Ok(())
    }

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
