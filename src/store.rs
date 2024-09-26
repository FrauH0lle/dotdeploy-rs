//! This module manages the tracking of the deployment process.
//!
//! Provided functionality include the creation of checksums for files, backing them up, storing
//! their source and target as well as their associated module.

pub(crate) mod backups;
pub(crate) mod checksums;
pub(crate) mod db;
pub(crate) mod errors;
pub(crate) mod files;
pub(crate) mod init;
pub(crate) mod modules;

#[cfg(test)]
pub(crate) mod tests;
