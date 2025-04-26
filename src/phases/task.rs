use crate::config::DotdeployConfig;
use crate::logs::log_output;
use crate::modules::{self, DeployPhase};
use crate::utils::commands::exec_output;
use crate::utils::sudo::PrivilegeManager;
use color_eyre::eyre::eyre;
use color_eyre::{Report, Result};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::hash::Hash;
use std::str::FromStr;
use tracing::{debug, info, warn};
use uuid::Uuid;

#[derive(Debug, Default, Deserialize, Serialize, Hash)]
pub(crate) struct PhaseTask {
    pub(crate) module_name: String,
    pub(crate) setup: Vec<PhaseTaskDefinition>,
    pub(crate) config: Vec<PhaseTaskDefinition>,
    pub(crate) update: Vec<PhaseTaskDefinition>,
    pub(crate) remove: Vec<PhaseTaskDefinition>,
    pub(crate) description: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize, Hash)]
pub(crate) struct PhaseTaskDefinition {
    pub(crate) description: Option<String>,
    pub(crate) shell: Option<OsString>,
    pub(crate) exec: Option<OsString>,
    pub(crate) args: Option<Vec<OsString>>,
    pub(crate) expand_args: bool,
    pub(crate) sudo: bool,
    pub(crate) hook: PhaseHook,
}

#[derive(Debug, Default, PartialEq, Deserialize, Serialize, Hash)]
pub(crate) enum PhaseHook {
    Pre,
    #[default]
    Post,
}

impl PhaseTask {
    pub(crate) async fn exec(
        &self,
        pm: &PrivilegeManager,
        config: &DotdeployConfig,
        phase: &DeployPhase,
        hook: PhaseHook,
    ) -> Result<()> {
        let tasks = match phase {
            DeployPhase::Setup => &self.setup,
            DeployPhase::Config => &self.config,
            DeployPhase::Update => &self.update,
            DeployPhase::Remove => &self.remove,
        };

        for task in tasks.iter().filter(|t| t.hook == hook) {
            if let Some(description) = &self.description {
                if let Some(sub_description) = &task.description {
                    info!("{}: {}", description, sub_description);
                } else {
                    info!(
                        "{}: Running {}",
                        description,
                        match phase {
                            DeployPhase::Setup => "setup task",
                            DeployPhase::Config => "config task",
                            DeployPhase::Update => "update task",
                            DeployPhase::Remove => "remove task",
                        }
                    );
                }
            } else {
                if let Some(sub_description) = &task.description {
                    info!("{}", sub_description);
                }
            }

            task.exec(&self.module_name, pm, config).await?
        }
        Ok(())
    }

    pub(crate) async fn calculate_uuid(&self) -> Result<Uuid> {
        // First serialize the struct
        let serialized = serde_json::to_string(self)?;

        // Create a UUID v5 (using a namespace and the serialized content)
        let namespace = Uuid::NAMESPACE_OID;
        Ok(Uuid::new_v5(&namespace, serialized.as_bytes()))
    }
}

impl PhaseTaskDefinition {
    async fn exec(
        &self,
        module_name: &str,
        pm: &PrivilegeManager,
        config: &DotdeployConfig,
    ) -> Result<()> {
        let module_path = modules::locate_module(module_name, config)?;

        if let Some(ref shell) = self.shell {
            let shell_display = format!(
                "{}...",
                shell
                    .to_string_lossy()
                    .replace(['\n', '\r'], " ")
                    .chars()
                    .take(50)
                    .collect::<String>()
            );
            debug!("Executing `{}`", &shell_display);

            let shell_args = [&OsString::from_str("-c")?, shell];
            let output = temp_env::async_with_vars(
                [("DOD_CURRENT_MODULE", Some(module_path))],
                exec_output(&OsString::from_str("sh")?, shell_args),
            )
            .await?;

            log_output!(output.stdout, "Stdout", &shell_display, info);
            log_output!(output.stderr, "Stderr", &shell_display, info);

            if output.status.success() {
                Ok(())
            } else {
                Err(eyre!(
                    "Failed to execute {} from module {}",
                    shell.to_string_lossy(),
                    module_name
                ))
            }
        } else if let Some(ref exec) = self.exec {
            let original_args = self.args.as_deref().unwrap_or(&[]);
            let args = if self.expand_args && !original_args.is_empty() {
                temp_env::with_var("DOD_CURRENT_MODULE", Some(&module_path), || {
                    original_args
                        .iter()
                        .map(|arg| {
                            shellexpand::path::full(arg)
                                .map(|s| s.into_owned().into_os_string())
                                .map_err(|e| {
                                    eyre!(
                                        "Failed to expand argument '{}': {}",
                                        arg.to_string_lossy(),
                                        e
                                    )
                                })
                        })
                        .collect::<Result<Vec<OsString>, Report>>()
                })?
            } else {
                original_args.to_vec()
            };
            let args_display = args
                .iter()
                .map(|a| a.to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join(" ");
            let exec_display = format!("{} {}", exec.to_string_lossy(), args_display);

            if self.sudo {
                warn!("Executing `{}` with elevated permissions", exec_display);
            } else {
                debug!("Executing `{}`", exec_display);
            }

            if self.sudo {
                let output = temp_env::async_with_vars(
                    [("DOD_CURRENT_MODULE", Some(module_path))],
                    pm.sudo_exec_output(exec, &args, Some(&format!("Executing {}", exec_display,))),
                )
                .await?;

                log_output!(output.stdout, "Stdout", exec_display, info);
                log_output!(output.stderr, "Stderr", exec_display, info);

                if output.status.success() {
                    Ok(())
                } else {
                    return Err(eyre!(
                        "Failed to execute {} from module {}",
                        exec_display,
                        module_name
                    ));
                }
            } else {
                let output = temp_env::async_with_vars(
                    [("DOD_CURRENT_MODULE", Some(module_path))],
                    exec_output(exec, &args),
                )
                .await?;

                log_output!(output.stdout, "Stdout", exec_display, info);
                log_output!(output.stderr, "Stderr", exec_display, info);

                if output.status.success() {
                    Ok(())
                } else {
                    return Err(eyre!(
                        "Failed to execute {} from module {}",
                        exec_display,
                        module_name
                    ));
                }
            }
        } else {
            Ok(())
        }
    }
}
