//! This module provides various utility functions needed throughout dotdeploy.
//!
//! These include file operations like copy or link, manipulating file metadata and permissions as
//! well as elevating privileges.

pub(crate) mod common;
pub(crate) mod file_checksum;
pub(crate) mod file_fs;
pub(crate) mod file_metadata;
pub(crate) mod file_permissions;
pub(crate) mod sudo;
