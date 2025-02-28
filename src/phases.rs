use crate::config::DotdeployConfig;
use crate::modules::files::ModuleFile;
use crate::modules::tasks::ModuleTask;
use crate::phases::file::PhaseFile;
use crate::phases::task::PhaseTask;
use crate::store::Stores;
use crate::store::sqlite_files::StoreFile;
use crate::utils::FileUtils;
use crate::utils::file_fs;
use crate::utils::file_metadata::FileMetadata;
use crate::utils::file_permissions;
use crate::utils::sudo::PrivilegeManager;
use color_eyre::eyre::{WrapErr, eyre};
use color_eyre::{Report, Result, Section};
use handlebars::Handlebars;
use task::PhaseHook;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio::task::JoinSet;
use toml::Value;
use tracing::{debug, info, instrument, warn};

pub(crate) mod file;
pub(crate) mod task;

#[derive(Debug, Default)]
pub(crate) struct DeployPhaseStruct {
    pub(crate) files: Vec<PhaseFile>,
    pub(crate) tasks: Vec<PhaseTask>,
}

impl DeployPhaseStruct {
    pub(crate) async fn deploy_files(
        &mut self,
        pm: Arc<PrivilegeManager>,
        stores: Arc<Stores>,
        context: Arc<HashMap<String, Value>>,
        hb: Arc<Handlebars<'static>>,
    ) -> Result<()> {
        let mut set = JoinSet::new();

        for file in self.files.drain(..) {
            set.spawn({
                let pm = Arc::clone(&pm);
                let stores = Arc::clone(&stores);
                let hb = Arc::clone(&hb);
                let context = Arc::clone(&context);
                async move {
                    file.deploy(pm, stores, context, hb).await?;
                    Ok::<_, Report>(())
                }
            });
        }
        let results = set.join_all().await;
        if results.iter().any(|r| r.is_err()) {
            // Collect and combine errors
            let err = results
                .into_iter()
                .filter(Result::is_err)
                .map(Result::unwrap_err)
                .fold(eyre!("Failed to process modules"), |report, e| {
                    report.with_error(|| crate::errors::StrError(format!("{:?}", e)))
                });

            return Err(err);
        }

        Ok(())
    }

    pub(crate) async fn exec_pre_tasks(
        &mut self,
        pm: &PrivilegeManager,
        config: &DotdeployConfig
    ) -> Result<()> {
        for task in self.tasks.iter().filter(|t| t.hook == PhaseHook::Pre) {
            task.exec(pm, config).await?;
        }

        Ok(())
    }

    pub(crate) async fn exec_post_tasks(
        &mut self,
        pm: &PrivilegeManager,
        config: &DotdeployConfig
    ) -> Result<()> {
        for task in self.tasks.iter().filter(|t| t.hook == PhaseHook::Post) {
            task.exec(pm, config).await?;
        }

        Ok(())
    }

}
