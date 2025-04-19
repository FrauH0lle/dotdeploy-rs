use crate::config::DotdeployConfig;
use crate::errors;
use crate::modules::DeployPhase;
use crate::phases::DeployPhaseTasks;
use crate::store::Store;
use crate::store::sqlite::SQLiteStore;
use crate::utils::sudo::PrivilegeManager;
use color_eyre::Result;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::task::JoinSet;
use tracing::info;

/// Update deployed modules and execute maintenance tasks
///
/// When called without specific modules, updates all deployed modules. Executes cached pre/post
/// tasks and displays any stored messages for updated modules.
///
/// * `modules` - Optional list of specific modules to update (all if None)
/// * `config` - Application configuration containing deployment settings
/// * `store` - User store instances to modify
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
    store: Arc<SQLiteStore>,
    pm: Arc<PrivilegeManager>,
) -> Result<bool> {
    let modules = if let Some(modules) = modules {
        modules.into_iter().collect::<HashSet<_>>()
    } else {
        store
            .get_all_modules()
            .await?
            .into_iter()
            .map(|m| m.name)
            .collect::<HashSet<_>>()
    };

    let mut set = JoinSet::new();
    for module in modules.into_iter() {
        let store = Arc::clone(&store);
        set.spawn(async move {
            let tasks = store.get_tasks(&module).await?;
            Ok((module, tasks))
        });
    }
    let (modules, tasks): (HashSet<_>, Vec<_>) = errors::join_errors(set.join_all().await)?
        .into_iter()
        .unzip();
    let mut tasks = DeployPhaseTasks {
        tasks: tasks.into_iter().flatten().collect(),
    };

    tasks.exec_pre_tasks(&pm, &config, DeployPhase::Update).await?;
    tasks.exec_post_tasks(&pm, &config, DeployPhase::Update).await?;

    for m in modules {
        let msgs = store.get_all_cached_messages(m.as_str(), "update").await;

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
