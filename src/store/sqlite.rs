use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{bail, Context, Result};
use sqlx::sqlite;

use crate::{store::Store, utils};

/// Representation of the store database
#[derive(Clone, Debug)]
pub(crate) struct SQLiteStore {
    /// SQLite connection pool
    pub(crate) pool: Option<sqlite::SqlitePool>,
    /// Store location
    pub(crate) path: PathBuf,
    /// Indicates whether this is a system-wide store (true) or user-specific store (false)
    pub(crate) system: bool,
}

impl Store for SQLiteStore {
    async fn init(&mut self) -> Result<()> {
        // Create the directory if it doesn't exist
        self.create_dir().await?;

        // Set the full path for the SQLite database file
        self.path = self.path.join("store.sqlite");

        let database_url = format!("sqlite://{}", utils::file_fs::path_to_string(&self.path)?);
        let pool_timeout = std::time::Duration::from_secs(30);
        let max_connections =
            u32::try_from(num_cpus::get_physical() * 4).expect("Could not convert usize to u32");
        let connection_options = sqlite::SqliteConnectOptions::from_str(&database_url)?
            .create_if_missing(true)
            .journal_mode(sqlite::SqliteJournalMode::Wal)
            .synchronous(sqlite::SqliteSynchronous::Normal)
            .busy_timeout(pool_timeout);

        let pool = sqlite::SqlitePoolOptions::new()
            .max_connections(max_connections)
            .connect_with(connection_options)
            .await?;
        // // Create the connection pool
        // let pool = Config::new(&self.path)
        //     .create_pool(Runtime::Tokio1)
        //     .with_context(|| {
        //         format!("Failed to create pool for store database {:?}", &self.path)
        //     })?;

        // let conn = pool
        //     .get()
        //     .await
        //     .with_context(|| format!("Failed to connect to store database {:?}", &self.path))?;

        // // Initialize the database with optimal settings
        // conn.interact(move |conn| -> Result<(), SQLiteError>> {
        //     prepare_connection(conn).map_err(|e| e.into_anyhow())?;
        //     Ok(())
        // })
        // .await
        // .map_err(|e| e.into_anyhow())??;

        // // Create the necessary tables
        // self.create_tables(&conn)
        //     .await
        //     .map_err(|e| e.into_anyhow())?;

        // Store the initialized pool
        self.pool = Some(pool);
        Ok(())
    }

    fn path(&self) -> &PathBuf {
        &self.path
    }

    fn is_system(&self) -> bool {
        self.system
    }
}
