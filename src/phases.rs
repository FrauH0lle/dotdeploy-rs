use crate::config::DotdeployConfig;
use crate::phases::file::PhaseFile;
use crate::phases::task::PhaseTask;
use crate::store::sqlite::SQLiteStore;
use crate::utils::sudo::PrivilegeManager;
use color_eyre::{Report, Result};
use handlebars::Handlebars;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use task::PhaseHook;
use tokio::task::JoinSet;
use toml::Value;

pub(crate) mod file;
pub(crate) mod task;

#[derive(Debug, Default, Deserialize, Serialize)]
pub(crate) struct DeployPhaseStruct {
    pub(crate) files: Vec<PhaseFile>,
    pub(crate) tasks: Vec<PhaseTask>,
}

impl DeployPhaseStruct {
    pub(crate) async fn deploy_files(
        &mut self,
        pm: Arc<PrivilegeManager>,
        store: Arc<SQLiteStore>,
        context: Arc<HashMap<String, Value>>,
        hb: Arc<Handlebars<'static>>,
    ) -> Result<()> {
        let mut set = JoinSet::new();

        for file in self.files.drain(..) {
            set.spawn({
                let pm = Arc::clone(&pm);
                let store = Arc::clone(&store);
                let hb = Arc::clone(&hb);
                let context = Arc::clone(&context);
                async move {
                    file.deploy(pm, store, context, hb).await?;
                    Ok::<_, Report>(())
                }
            });
        }

        crate::errors::join_errors(set.join_all().await)?;

        Ok(())
    }

    pub(crate) async fn exec_pre_tasks(
        &mut self,
        pm: &PrivilegeManager,
        config: &DotdeployConfig,
    ) -> Result<()> {
        for task in self.tasks.iter().filter(|t| t.hook == PhaseHook::Pre) {
            task.exec(pm, config).await?;
        }

        Ok(())
    }

    pub(crate) async fn exec_post_tasks(
        &mut self,
        pm: &PrivilegeManager,
        config: &DotdeployConfig,
    ) -> Result<()> {
        for task in self.tasks.iter().filter(|t| t.hook == PhaseHook::Post) {
            task.exec(pm, config).await?;
        }

        Ok(())
    }
}
