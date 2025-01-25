//! This module provides functionality for managing modules in the dotdeploy store database. It
//! includes operations for adding, removing, and retrieving module information.

use deadpool_sqlite::rusqlite::{params, OptionalExtension};

use crate::store::db;
use crate::store::errors::SQLiteError;

/// Representation of a store module entry (row) in the database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StoreModule {
    /// The name of the module
    pub(crate) name: String,
    /// The location of the module
    pub(crate) location: String,
    /// The user associated with the module (optional)
    pub(crate) user: Option<String>,
    /// The reason for adding the module
    pub(crate) reason: String,
    /// Dependencies of the module (optional)
    pub(crate) depends: Option<String>,
    /// The date and time when the module was added or last updated
    pub(crate) date: chrono::DateTime<chrono::Local>,
}

impl db::Store {
    /// Adds or updates a module in the database.
    ///
    /// If a module with the same name already exists, this operation will be ignored.
    ///
    /// # Arguments
    /// * `module` - The `StoreModule` to be added or updated.
    ///
    /// # Returns
    /// * `Ok(())` if the operation is successful.
    /// * `Err(SQLiteError)` if there's an error during the database operation.
    pub(crate) async fn add_module(&self, module: StoreModule) -> Result<(), SQLiteError> {
        let conn = &self.get_con().await?;
        conn.interact(move |conn| -> Result<(), SQLiteError> {
            db::prepare_connection(conn)?;
            conn.execute(
                "INSERT INTO modules (name, location, user, reason, depends, date)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT(name) DO NOTHING",
                params![
                    module.name,
                    module.location,
                    module.user,
                    module.reason,
                    module.depends,
                    module.date
                ],
            )?;
            Ok(())
        })
        .await??;
        Ok(())
    }

    /// Removes a module from the database.
    ///
    /// # Arguments
    /// * `module` - The name of the module to be removed.
    ///
    /// # Returns
    /// * `Ok(())` if the operation is successful.
    /// * `Err(SQLiteError)` if there's an error during the database operation.
    pub(crate) async fn remove_module<S: AsRef<str>>(&self, module: S) -> Result<(), SQLiteError> {
        let module = module.as_ref().to_owned();
        let conn = &self.get_con().await?;
        conn.interact(move |conn| -> Result<(), SQLiteError> {
            db::prepare_connection(conn)?;
            conn.execute("DELETE FROM modules WHERE name = $1", params![module])?;
            Ok(())
        })
        .await??;

        Ok(())
    }

    /// Retrieves a single module from the store by its name.
    ///
    /// # Arguments
    /// * `name` - The name of the module to retrieve.
    ///
    /// # Returns
    /// * `Ok(StoreModule)` if the module is found.
    /// * `Err(SQLiteError)` if there's an error during the database operation or if the module is
    ///   not found.
    pub(crate) async fn get_module<S: AsRef<str>>(
        &self,
        name: S,
    ) -> Result<StoreModule, SQLiteError> {
        let name = name.as_ref().to_owned();
        let conn = &self.get_con().await?;

        conn.interact(move |conn| -> Result<StoreModule, SQLiteError> {
            db::prepare_connection(conn)?;
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

    /// Retrieves all modules from the store.
    ///
    /// # Returns
    /// * `Ok(Vec<StoreModule>)` containing all modules in the database.
    /// * `Err(SQLiteError)` if there's an error during the database operation.
    pub(crate) async fn get_all_modules(&self) -> Result<Vec<StoreModule>, SQLiteError> {
        let conn = &self.get_con().await?;

        conn.interact(move |conn| -> Result<Vec<StoreModule>, SQLiteError> {
            db::prepare_connection(conn)?;
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

            // Process the query results, handling any errors
            let mut modules = Vec::with_capacity(rows.len());
            for row in rows {
                match row {
                    Ok(module) => modules.push(module),
                    Err(e) => eprintln!("Error processing module row: {:?}", e),
                }
            }
            Ok(modules)
        })
        .await?
    }
}
