use crate::config::DotdeployConfig;
use crate::logs::log_output;
use crate::modules;
use crate::utils::commands::exec_output;
use crate::utils::sudo::PrivilegeManager;
use color_eyre::eyre::eyre;
use color_eyre::{Report, Result};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::str::FromStr;
use tracing::{info, warn};

#[derive(Debug, Default, Deserialize, Serialize)]
pub(crate) struct PhaseTask {
    pub(crate) module_name: String,
    pub(crate) shell: Option<OsString>,
    pub(crate) exec: Option<OsString>,
    pub(crate) args: Option<Vec<OsString>>,
    pub(crate) expand_args: bool,
    pub(crate) sudo: bool,
    pub(crate) hook: PhaseHook,
}

#[derive(Debug, Default, PartialEq, Deserialize, Serialize)]
pub(crate) enum PhaseHook {
    Pre,
    #[default]
    Post,
}

impl PhaseTask {
    pub(crate) async fn exec(&self, pm: &PrivilegeManager, config: &DotdeployConfig) -> Result<()> {
        let module_path = modules::locate_module(&self.module_name, config)?;

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
            info!("Executing `{}`", &shell_display);

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
                    &self.module_name
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
                info!("Executing `{}`", exec_display);
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
                        &self.module_name
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
                        &self.module_name
                    ));
                }
            }
        } else {
            Ok(())
        }
    }
}
