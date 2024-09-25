//! This module provides functionality for managing SQLite database connections and operations for
//! the dotdeploy application's store.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use deadpool_sqlite::{Config, Runtime};

use crate::store::errors::SQLiteError;
use crate::utils::file_fs;
use crate::utils::sudo;

/// Representation of the store database
#[derive(Clone, Debug)]
pub(crate) struct Store {
    /// SQLite connection pool
    pub(crate) pool: Option<deadpool_sqlite::Pool>,
    /// Store location
    pub(crate) path: PathBuf,
    /// Indicates whether this is a system-wide store (true) or user-specific store (false)
    system: bool,
}

/// Runs maintenance and closes the connection gracefully, cleaning up temporary WAL and SHM files.
///
/// This function attempts to clean up the Write-Ahead Logging (WAL) files that SQLite creates
/// during normal operation. It runs in a loop, checking for and cleaning up these files until they
/// no longer exist.
///
/// # Arguments
/// * `path` - A reference to the path of the SQLite database file.
///
/// # Returns
/// * `Ok(())` if the cleanup is successful.
/// * `Err(anyhow::Error)` if an error occurs during the cleanup process.
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

            // Set journal mode to WAL
            conn.pragma_update(
                Some(deadpool_sqlite::rusqlite::DatabaseName::Main),
                "journal_mode",
                "WAL",
            )
            .context("Failed to run PRAGMA journal_mode=WAL")?;

            // Set synchronous mode to NORMAL for better performance
            conn.pragma_update(
                Some(deadpool_sqlite::rusqlite::DatabaseName::Main),
                "synchronous",
                "NORMAL",
            )
            .context("Failed to run PRAGMA synchronous=NORMAL")?;

            // Run VACUUM to optimize the database
            conn.execute_batch("VACUUM;;")
                .context("Failed to run VACUUM;")?;

            // Close the connection and wait a bit before checking again
            drop(conn);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    Ok(())
}

/// Prepares a SQLite connection with optimal settings.
///
/// This function sets the journal mode to WAL (Write-Ahead Logging) and the synchronous mode to
/// NORMAL, which can improve performance in many scenarios.
///
/// # Arguments
/// * `connection` - A mutable reference to the SQLite connection to be prepared.
///
/// # Returns
/// * `Ok(())` if the connection is successfully prepared.
/// * `Err(SQLiteError)` if an error occurs while setting the pragmas.
pub(crate) fn prepare_connection(
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
    /// Creates a new Store configuration but does not initialize the connection pool yet.
    ///
    /// # Arguments
    /// * `path` - The path where the store database will be created.
    /// * `system` - A boolean indicating whether this is a system-wide store (true) or
    ///   user-specific store (false).
    ///
    /// # Returns
    /// A new `Store` instance with the specified path and system flag.
    pub(crate) fn new(path: PathBuf, system: bool) -> Self {
        Store {
            pool: None,
            path,
            system,
        }
    }

    /// Creates the directory for the store if it doesn't exist.
    ///
    /// For system stores, this method uses sudo to create the directory and set appropriate
    /// permissions. For user stores, it creates the directory without elevated permissions.
    ///
    /// # Returns
    /// * `Ok(())` if the directory is successfully created or already exists.
    /// * `Err(anyhow::Error)` if an error occurs during directory creation.
    async fn create_dir(&self) -> Result<()> {
        if self.system {
            self.create_system_dir().await
        } else {
            self.create_user_dir().await
        }
    }

    /// Creates the directory for a system-wide store.
    async fn create_system_dir(&self) -> Result<()> {
        match self.path.try_exists() {
            Ok(false) => {
                debug!(
                    "Store directory '{}' does not exist, creating.",
                    &self.path.display()
                );

                // Create the directory with sudo
                file_fs::ensure_dir_exists(&self.path)
                    .await
                    .with_context(|| format!("Failed to create directory {:?}", &self.path))?;

                // Set permissions to allow all users to write to the directory
                sudo::sudo_exec(
                    "chmod",
                    &["777", file_fs::path_to_string(&self.path)?.as_str()],
                    Some("Adjusting permissions of system store DB directory"),
                )
                .await
                .with_context(|| {
                    format!("Failed to change permissions of directory {:?}", &self.path)
                })?;

                Ok(())
            }
            Ok(true) => {
                debug!(
                    "Store directory '{}' exists already, continuing.",
                    &self.path.display()
                );
                Ok(())
            }
            Err(e) => bail!("{}", e),
        }
    }

    /// Creates the directory for a user-specific store.
    async fn create_user_dir(&self) -> Result<()> {
        match self.path.try_exists() {
            Ok(false) => {
                debug!(
                    "Store directory '{}' does not exist, creating.",
                    &self.path.display()
                );
                file_fs::ensure_dir_exists(&self.path)
                    .await
                    .with_context(|| format!("Failed to create directory {:?}", &self.path))?;
                Ok(())
            }
            Ok(true) => {
                debug!(
                    "Store directory '{}' exists already, continuing.",
                    &self.path.display()
                );
                Ok(())
            }
            Err(e) => bail!("{}", e),
        }
    }

    /// Initializes a store database.
    ///
    /// This method creates the necessary directory, initializes the SQLite database, creates the
    /// required tables, and sets up the connection pool.
    ///
    /// # Returns
    /// * `Ok(())` if the initialization is successful.
    /// * `Err(SQLiteError)` if an error occurs during initialization.
    pub(crate) async fn init(&mut self) -> Result<(), SQLiteError> {
        // Create the directory if it doesn't exist
        self.create_dir().await.map_err(SQLiteError::Other)?;

        // Set the full path for the SQLite database file
        self.path = self.path.join("store.sqlite");

        // Create the connection pool
        let pool = Config::new(&self.path)
            .create_pool(Runtime::Tokio1)
            .with_context(|| {
                format!("Failed to create pool for store database {:?}", &self.path)
            })?;

        let conn = pool
            .get()
            .await
            .with_context(|| format!("Failed to connect to store database {:?}", &self.path))?;

        // Initialize the database with optimal settings
        conn.interact(move |conn| -> Result<(), SQLiteError> {
            prepare_connection(conn)?;
            Ok(())
        })
        .await??;

        // Create the necessary tables
        self.create_tables(&conn).await?;

        // Store the initialized pool
        self.pool = Some(pool);
        Ok(())
    }

    /// Creates the necessary tables in the SQLite database.
    async fn create_tables(&self, conn: &deadpool_sqlite::Object) -> Result<(), SQLiteError> {
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

        Ok(())
    }

    /// Closes all connections to the pool and performs cleanup operations.
    ///
    /// # Returns
    /// * `Ok(())` if all connections are successfully closed.
    /// * `Err(SQLiteError)` if an error occurs during the closing process.
    pub(crate) async fn close(&self) -> Result<(), SQLiteError> {
        if let Some(pool) = &self.pool {
            // Wait for all ongoing database tasks to complete
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

    /// Returns a connection from the store's connection pool.
    ///
    /// # Returns
    /// * `Ok(deadpool_sqlite::Object)` if a connection is successfully obtained.
    /// * `Err(SQLiteError)` if the pool is not initialized or a connection cannot be obtained.
    pub(crate) async fn get_con(&self) -> Result<deadpool_sqlite::Object, SQLiteError> {
        if let Some(pool) = &self.pool {
            Ok(pool.get().await?)
        } else {
            Err(anyhow!("Store database not initialized!").into())
        }
    }
}
