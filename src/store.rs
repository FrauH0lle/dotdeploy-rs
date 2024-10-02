//! This module manages the tracking of the deployment process.
//!
//! Provided functionality include the creation of checksums for files, backing them up, storing
//! their source and target as well as their associated module.

use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};

use crate::DEPLOY_SYSTEM_FILES;
use self::db::Store;
use self::init::{init_user_store, init_system_store};

pub(crate) mod backups;
pub(crate) mod checksums;
pub(crate) mod db;
pub(crate) mod errors;
pub(crate) mod files;
pub(crate) mod init;
pub(crate) mod modules;

#[cfg(test)]
pub(crate) mod tests;

pub(crate) struct Stores {
    pub(crate) user_store: Store,
    pub(crate) system_store: Option<Store>,
}

impl Stores {
    pub(crate) async fn init() -> Result<Self> {
        Ok(Self {
            user_store: init_user_store(None)
                .await
                .map_err(|e| e.into_anyhow())
                .context("Failed to initialize user store")?,
            system_store: if DEPLOY_SYSTEM_FILES.load(Ordering::Relaxed) {
                Some(
                    init_system_store()
                        .await
                        .map_err(|e| e.into_anyhow())
                        .context("Failed to initialize system store")?,
                )
            } else {
                None
            },
        })
    }
}
