//! This module provides functionality for managing modules in the dotdeploy store database. It
//! includes operations for adding, removing, and retrieving module information.

/// Representation of a store module entry (row) in the database.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
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
    pub(crate) date: chrono::DateTime<chrono::Utc>,
}

impl StoreModule {
    pub (crate) fn new(
        name: String,
        location: String,
        user: Option<String>,
        reason: String,
        depends: Option<String>,
        date: chrono::DateTime<chrono::Utc>,
    ) -> Self {
        StoreModule {
            name,
            location,
            user,
            reason,
            depends,
            date,
        }
    }
}
