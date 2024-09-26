//! This module defines the structure and operations for managing Dotdeploy modules.

pub(crate) mod actions;
pub(crate) mod conditional;
pub(crate) mod config;
pub(crate) mod files;
pub(crate) mod generate;
pub(crate) mod messages;
pub(crate) mod packages;
pub(crate) mod queue;

use std::cmp::Ordering;
use std::path::PathBuf;

use self::config::ModuleConfig;

/// Represents a Dotdeploy module with its properties and configuration.
#[derive(Debug)]
pub(crate) struct Module {
    /// The name of the module
    pub(crate) name: String,
    /// The file system path to the module
    pub(crate) location: PathBuf,
    /// The reason for adding this module (e.g., "manual" or "automatic")
    pub(crate) reason: String,
    /// The parsed configuration of the module
    pub(crate) config: ModuleConfig,
}

impl Eq for Module {}

impl PartialEq for Module {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl PartialOrd for Module {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Module {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name.cmp(&other.name)
    }
}
