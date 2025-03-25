use crate::store::Store;
use crate::{config::DotdeployConfig, store::Stores, utils::sudo::PrivilegeManager};
use color_eyre::Result;
use std::collections::HashSet;
use std::sync::Arc;
use tracing::info;

/// Update deployed modules and execute maintenance tasks
///
/// When called without specific modules, updates all deployed modules. Executes cached pre/post
/// tasks and displays any stored messages for updated modules.
///
/// * `modules` - Optional list of specific modules to update (all if None)
/// * `config` - Application configuration containing deployment settings
/// * `stores` - Combined user/system store instances to modify
/// * `pm` - Privilege manager for handling elevated permissions
///
/// # Errors
/// Returns errors if:
/// * Store database access fails
/// * Task execution fails
/// * Message retrieval fails
pub(crate) async fn update(
    modules: Option<Vec<String>>,
    config: Arc<DotdeployConfig>,
    stores: Arc<Stores>,
    pm: Arc<PrivilegeManager>,
) -> Result<bool> {
    // REVIEW 2025-03-23: Allow modules arg?
    let modules = if let Some(modules) = modules {
        modules.into_iter().collect::<HashSet<_>>()
    } else {
        stores
            .get_all_modules()
            .await?
            .into_iter()
            .map(|m| m.name)
            .collect::<HashSet<_>>()
    };

    if let Some(mut cached_update_tasks) = stores.user_store.get_cached_commands("update").await? {
        cached_update_tasks
            .tasks
            .retain(|t| modules.contains(&t.module_name));
        cached_update_tasks.exec_pre_tasks(&pm, &config).await?;
        cached_update_tasks.exec_post_tasks(&pm, &config).await?;
    }

    for m in modules {
        let msgs = stores
            .user_store
            .get_all_cached_messages(m.as_str(), "update")
            .await;

        if let Ok(msgs) = msgs {
            for msg in msgs.into_iter() {
                match msg.on_command.as_deref() {
                    Some("update") => {
                        info!("Message for {}:\n{}", msg.module_name, msg.message)
                    }
                    _ => unreachable!(),
                }
            }
        }
    }
    Ok(true)
}
