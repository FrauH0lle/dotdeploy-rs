//! This module handles checksum operations for files in the dotdeploy store.
//!
//! It provides functionality to retrieve and manage checksums for both source and destination
//! files, which is crucial for tracking file changes and ensuring data integrity during the dotfile
//! deployment process.

use std::path::Path;

use anyhow::{anyhow, Result};
use deadpool_sqlite::rusqlite::{params, OptionalExtension};

use crate::store::db;
use crate::store::errors::SQLiteError;
use crate::utils::file_fs;

// TODO 2024-09-25: Use a struct for the query result

impl db::Store {
    /// Retrieves the checksum of either a source or destination file from the store database.
    ///
    /// # Arguments
    ///
    /// * `filename` - The name of the file to retrieve the checksum for.
    /// * `which` - Specifies whether to retrieve the "source" or "destination" checksum.
    ///
    /// # Returns
    ///
    /// Returns `Ok(Some((String, String)))` with the file path and its checksum if found, or
    /// `Ok(None)` if not found. Returns an error if there's a database issue.
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
                    // Prepare the database connection
                    db::prepare_connection(conn)?;

                    // Prepare the SQL query based on whether we're looking for source or destination
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

                    // Execute the query and return the result
                    Ok(stmt
                       .query_row(params![filename], |row| Ok((row.get(0)?, row.get(1)?)))
                       .optional()?)
                },
            )
            .await??;

        // Process the query result
        // If both file and checksum are present, return them as a tuple
        // Otherwise, return None
        let result = match query {
            Some((Some(file), Some(checksum))) => Some((file, checksum)),
            _ => None,
        };

        Ok(result)
    }

    /// Retrieves the checksum of a source file from the store database.
    ///
    /// # Arguments
    ///
    /// * `filename` - The path of the file to retrieve the checksum for.
    ///
    /// # Returns
    ///
    /// Returns `Ok(Some((String, String)))` with the file path and its checksum if found, or
    /// `Ok(None)` if not found. Returns an error if there's a database issue.
    pub(crate) async fn get_source_checksum<P: AsRef<Path>>(
        &self,
        filename: P,
    ) -> Result<Option<(String, String)>, SQLiteError> {
        // Convert the path to a string, handling potential conversion errors
        let filename_str = file_fs::path_to_string(filename)?;

        // Call the generic get_file_checksum method with "source" parameter
        // This retrieves the checksum for the source file
        self.get_file_checksum(filename_str, "source").await
    }

    /// Retrieves the checksum of a destination file from the store database.
    ///
    /// # Arguments
    ///
    /// * `filename` - The path of the file to retrieve the checksum for.
    ///
    /// # Returns
    ///
    /// Returns `Ok(Some((String, String)))` with the file path and its checksum if found, or
    /// `Ok(None)` if not found. Returns an error if there's a database issue.
    pub(crate) async fn get_destination_checksum<P: AsRef<Path>>(
        &self,
        filename: P,
    ) -> Result<Option<(String, String)>, SQLiteError> {
        // Convert the path to a string, handling potential conversion errors
        let filename_str = file_fs::path_to_string(filename)?;

        // Call the generic get_file_checksum method with "destination" parameter
        // This retrieves the checksum for the destination file
        self.get_file_checksum(filename_str, "destination").await
    }

    /// Retrieves all checksums of either source or destination files from the store database.
    ///
    /// # Arguments
    ///
    /// * `which` - A string specifying whether to retrieve "source" or "destination" checksums.
    ///
    /// # Returns
    ///
    /// Returns a vector of tuples, each containing an optional file path and its optional checksum.
    /// Returns an error if there's a database issue or if an invalid `which` parameter is provided.
    async fn get_all_checksums(
        &self,
        which: &str,
    ) -> Result<Vec<(Option<String>, Option<String>)>, SQLiteError> {
        let which = which.to_owned();
        let conn = &self.get_con().await?;

        let query = conn
            .interact(
                move |conn| -> Result<Vec<(Option<String>, Option<String>)>, SQLiteError> {
                    // Prepare the database connection for the query
                    db::prepare_connection(conn)?;

                    // Prepare the SQL query based on whether we're looking for source or
                    // destination
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

                    // Execute the query and collect the results
                    let rows: Vec<
                        Result<(Option<String>, Option<String>), deadpool_sqlite::rusqlite::Error>,
                    > = stmt
                        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                        .collect();

                    // Process the query results, handling any errors
                    let mut checksums = Vec::with_capacity(rows.len());
                    for row in rows {
                        match row {
                            Ok(row) => checksums.push(row),
                            Err(e) => eprintln!("Error processing row: {:?}", e), // Log the error
                        }
                    }

                    Ok(checksums)
                },
            )
            .await??;

        // Filter out entries where both the file and checksum are None. This ensures we only return
        // meaningful data.
        let result: Vec<(Option<String>, Option<String>)> = query
            .into_iter()
            .filter(|(file, checksum)| file.is_some() || checksum.is_some())
            .collect();

        Ok(result)
    }

    /// Retrieves all source checksums from the store database.
    ///
    /// # Returns
    ///
    /// Returns a vector of tuples, each containing an optional source file path and its optional
    /// checksum. Returns an error if there's a database issue.
    pub(crate) async fn get_all_src_checksums(
        &self,
    ) -> Result<Vec<(Option<String>, Option<String>)>, SQLiteError> {
        // Call the generic get_all_checksums method with "source" parameter
        // This retrieves checksums for all source files in the database
        self.get_all_checksums("source").await
    }

    /// Retrieves all destination checksums from the store database.
    ///
    /// # Returns
    ///
    /// Returns a vector of tuples, each containing an optional destination file path and its
    /// optional checksum. Returns an error if there's a database issue.
    pub(crate) async fn get_all_dest_checksums(
        &self,
    ) -> Result<Vec<(Option<String>, Option<String>)>, SQLiteError> {
        // Call the generic get_all_checksums method with "destination" parameter
        // This retrieves checksums for all destination files in the database
        self.get_all_checksums("destination").await
    }
}

//
// Tests

#[cfg(test)]
mod tests {
    use super::*;

    use crate::store::tests::store_setup_helper;

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
}
