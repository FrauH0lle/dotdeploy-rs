//! This module defines custom error types for SQLite operations and provides utilities for error
//! handling and conversion.

use anyhow::anyhow;
use thiserror::Error;

/// Represents errors that can occur during SQLite operations.
#[derive(Error, Debug)]
pub(crate) enum SQLiteError {
    /// Error occurred while creating the connection pool.
    #[error("Failed to create pool")]
    CreatePoolFailed(#[from] deadpool_sqlite::CreatePoolError),

    /// Error occurred while executing an SQL statement.
    #[error("Failed to execute SQL statement")]
    QueryError(#[from] deadpool_sqlite::rusqlite::Error),

    /// Error occurred while interacting with the connection pool.
    #[error("Failed to interact with connection pool")]
    ConnectionInteractError(#[from] deadpool_sqlite::InteractError),

    /// Error occurred while attempting to get a connection from the pool.
    #[error("Failed to get connection")]
    GetConnectionError(#[from] deadpool_sqlite::PoolError),

    /// Error occurred when a query returned an invalid or unexpected result.
    #[error("Query returned invalid result")]
    InvalidQueryResult,

    /// Catch-all for other errors that don't fit into the above categories.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl SQLiteError {
    /// Converts `SQLiteError` to `anyhow::Error`, handling non-Sync errors.
    ///
    /// This method is necessary because `deadpool_sqlite::InteractError` doesn't implement `Sync`,
    /// making it incompatible with `anyhow::Error` in some contexts. This conversion ensures that
    /// all `SQLiteError` variants can be safely converted to `anyhow::Error`.
    ///
    /// # Returns
    /// An `anyhow::Error` representing the original `SQLiteError`.
    pub(crate) fn into_anyhow(self) -> anyhow::Error {
        match self {
            // Special handling for ConnectionInteractError to preserve error details
            SQLiteError::ConnectionInteractError(e) => {
                anyhow!("Connection interaction failed: {:?}", e)
            }
            // For all other error types, we can use the default Debug representation
            _ => anyhow!("{:?}", self),
        }
    }
}
