//! This module provides functionality for managing file entries in the dotdeploy store database.
//!
//! It includes operations for adding, removing, retrieving, and checking the existence of file
//! records.

use std::path::Path;

use deadpool_sqlite::rusqlite::params;

use crate::store::db;
use crate::store::errors::SQLiteError;
use crate::utils::file_fs;

/// Representation of a store file entry (row) in the database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StoreFile {
    /// The module associated with this file
    pub(crate) module: String,
    /// The source path of the file (optional)
    pub(crate) source: Option<String>,
    /// The checksum of the source file (optional)
    pub(crate) source_checksum: Option<String>,
    /// The destination path of the file
    pub(crate) destination: String,
    /// The checksum of the destination file (optional)
    pub(crate) destination_checksum: Option<String>,
    /// The operation performed on the file (must be either 'link', 'copy', or 'create')
    pub(crate) operation: String,
    /// The user associated with this file operation (optional)
    pub(crate) user: Option<String>,
    /// The date and time when the file entry was added or last updated
    pub(crate) date: chrono::DateTime<chrono::Local>,
}

impl db::Store {
    /// Retrieves a single file entry from the store based on its filename.
    ///
    /// # Arguments
    /// * `filename` - The path of the file to retrieve.
    ///
    /// # Returns
    /// * `Ok(StoreFile)` if the file is found.
    /// * `Err(SQLiteError)` if there's an error during the database operation or if the file is not
    ///   found.
    pub(crate) async fn get_file<P: AsRef<Path>>(
        &self,
        filename: P,
    ) -> Result<StoreFile, SQLiteError> {
        let filename_str = file_fs::path_to_string(filename)?;

        let conn = &self.get_con().await?;

        conn.interact(move |conn| -> Result<StoreFile, SQLiteError> {
            db::prepare_connection(conn)?;
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

    /// Adds or updates a single file entry in the database.
    ///
    /// # Arguments
    /// * `file` - The `StoreFile` to be added or updated.
    ///
    /// # Returns
    /// * `Ok(())` if the operation is successful.
    /// * `Err(SQLiteError)` if there's an error during the database operation.
    pub(crate) async fn add_file(&self, file: StoreFile) -> Result<(), SQLiteError> {
        let conn = &self.get_con().await?;

        // Retrieve the ID of the module
        let module_id = conn
            .interact(move |conn| -> Result<i64, SQLiteError> {
                db::prepare_connection(conn)?;
                Ok(conn.query_row(
                    "SELECT id FROM modules WHERE name = $1",
                    params![file.module],
                    |row| row.get(0),
                )?)
            })
            .await??;

        conn.interact(move |conn| -> Result<(), SQLiteError> {
            db::prepare_connection(conn)?;
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

    /// Removes a single file entry from the database.
    ///
    /// # Arguments
    /// * `file` - The destination path of the file to be removed.
    ///
    /// # Returns
    /// * `Ok(())` if the operation is successful.
    /// * `Err(SQLiteError)` if there's an error during the database operation.
    pub(crate) async fn remove_file<S: AsRef<str>>(&self, file: S) -> Result<(), SQLiteError> {
        let file = file.as_ref().to_owned();
        let conn = &self.get_con().await?;
        conn.interact(move |conn| -> Result<(), SQLiteError> {
            db::prepare_connection(conn)?;
            conn.execute("DELETE FROM files WHERE destination = $1", params![file])?;
            Ok(())
        })
        .await??;

        Ok(())
    }

    /// Retrieves all file entries associated with a specific module.
    ///
    /// # Arguments
    /// * `module` - The name of the module to retrieve files for.
    ///
    /// # Returns
    /// * `Ok(Vec<StoreFile>)` containing all files associated with the module.
    /// * `Err(SQLiteError)` if there's an error during the database operation.
    pub(crate) async fn get_all_files<S: AsRef<str>>(
        &self,
        module: S,
    ) -> Result<Vec<StoreFile>, SQLiteError> {
        let module = module.as_ref().to_owned();
        let conn = &self.get_con().await?;

        conn.interact(move |conn| -> Result<Vec<StoreFile>, SQLiteError> {
            db::prepare_connection(conn)?;
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

            // Process the query results, handling any errors
            let mut files = Vec::with_capacity(rows.len());
            for row in rows {
                match row {
                    Ok(file) => files.push(file),
                    Err(e) => eprintln!("Error processing file row: {:?}", e),
                }
            }
            Ok(files)
        }).await?
    }

    /// Checks if a file exists in the store database.
    ///
    /// # Arguments
    /// * `path` - The path of the file to check.
    ///
    /// # Returns
    /// * `Ok(bool)` - `true` if the file exists in the database, `false` otherwise.
    /// * `Err(SQLiteError)` if there's an error during the database operation.
    pub(crate) async fn check_file_exists<P: AsRef<Path>>(
        &self,
        path: P,
    ) -> Result<bool, SQLiteError> {
        let path_str = file_fs::path_to_string(path)?;
        let store_path = self.path.clone();

        debug!("Looking for {} in {}", &path_str, &self.path.display());

        let conn = &self.get_con().await?;

        let result = conn
            .interact(move |conn| -> Result<bool, SQLiteError> {
                db::prepare_connection(conn)?;
                let mut stmt =
                    conn.prepare("SELECT destination FROM files WHERE destination = $1")?;

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
}

//
// Tests

#[cfg(test)]
mod tests {
    use super::*;

    use std::env;

    use anyhow::Result;
    use tempfile::tempdir;

    use crate::store::init::init_user_store;
    use crate::store::modules::StoreModule;
    use crate::store::tests::store_setup_helper;

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
}
