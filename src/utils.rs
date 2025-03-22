//! This module provides various utility functions needed throughout dotdeploy.
//!
//! These include file operations like copy or link, manipulating file metadata and permissions as
//! well as elevating privileges.

use std::sync::Arc;
use sudo::PrivilegeManager;

pub(crate) mod common;
pub(crate) mod file_checksum;
pub(crate) mod file_fs;
pub(crate) mod file_metadata;
pub(crate) mod file_permissions;
pub(crate) mod sudo;
pub(crate) mod commands;

/// Provides methods for file operations which might require elevated privileges.
#[derive(Debug)]
pub(crate) struct FileUtils {
    privilege_manager: Arc<PrivilegeManager>,
}

impl FileUtils {
    /// Provide potentially elevated file operations.
    ///
    /// # Arguments
    /// * `privilege_manager` - A [`PrivilegeManager`] wrapped in an [`Arc`]
    pub(crate) fn new(privilege_manager: Arc<PrivilegeManager>) -> Self {
        Self { privilege_manager }
    }
}
