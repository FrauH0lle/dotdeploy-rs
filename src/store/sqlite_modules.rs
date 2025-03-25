//! This module provides functionality for managing modules in the dotdeploy store database. It
//! includes operations for adding, removing, and retrieving module information.

use derive_builder::Builder;

/// Representation of a store module entry (row) in the database.
#[derive(Clone, Debug, PartialEq, Eq, Default, Builder)]
#[builder(setter(prefix = "with"))]
pub(crate) struct StoreModule {
    /// The name of the module
    #[builder(setter(into))]
    pub(crate) name: String,
    /// The location of the module (human-readable)
    #[builder(setter(into))]
    pub(crate) location: String,
    /// The location of the module (byte vector)
    #[builder(setter(into))]
    pub(crate) location_u8: Vec<u8>,
    /// The user associated with the module (optional)
    pub(crate) user: Option<String>,
    /// The reason for adding the module
    #[builder(setter(into))]
    pub(crate) reason: String,
    /// Dependencies of the module (optional)
    pub(crate) depends: Option<Vec<String>>,
    /// The date and time when the module was added or last updated
    pub(crate) date: chrono::DateTime<chrono::Utc>,
}
