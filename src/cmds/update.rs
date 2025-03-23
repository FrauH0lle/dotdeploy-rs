use crate::cmds::common;
use crate::store::Store;
use crate::store::sqlite_files::StoreFile;
use crate::store::sqlite_modules::StoreModule;
use crate::utils::{FileUtils, file_fs};
use crate::{
    config::DotdeployConfig, modules::queue::ModulesQueueBuilder, store::Stores,
    utils::sudo::PrivilegeManager,
};
use color_eyre::eyre::{WrapErr, eyre};
use color_eyre::{Report, Result};
use handlebars::Handlebars;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::sync::Arc;
use tokio::task::JoinSet;
use toml::Value;
use tracing::{debug, info, instrument, warn};

pub(crate) async fn update(
    config: Arc<DotdeployConfig>,
    stores: Arc<Stores>,
    pm: Arc<PrivilegeManager>,
) -> Result<bool> {
    // REVIEW 2025-03-23: Allow modules arg?

    if let Some(mut cached_update_tasks) = stores.user_store.get_all_cached_tasks("update").await {
        cached_update_tasks.exec_pre_tasks(&pm, &config).await?;
        cached_update_tasks.exec_post_tasks(&pm, &config).await?;
    }

    let modules = stores
        .get_all_modules()
        .await?
        .into_iter()
        .map(|m| m.name)
        .collect::<HashSet<_>>();

    for m in modules {

        let msgs = stores
            .user_store
            .get_all_cached_messages(m.as_str(), "update")
            .await;

        match msgs {
            Ok(msgs) => {
                for msg in msgs.into_iter() {
                    match msg.on_command.as_deref() {
                        Some("update") => {
                            info!("Message for {}:\n{}", msg.module_name, msg.message)
                        }
                        _ => unreachable!(),
                    }
                }
            }
            Err(_) => (),
        }
    }
    Ok(true)
}
