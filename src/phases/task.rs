use crate::config::DotdeployConfig;
use crate::logs::log_output;
use crate::modules;
use crate::utils::commands::exec_output;
use crate::utils::sudo::PrivilegeManager;
use color_eyre::eyre::eyre;
use color_eyre::Result;
use tracing::{info, warn};

#[derive(Debug, Default)]
pub(crate) struct PhaseTask {
    pub(crate) module_name: String,
    pub(crate) shell: Option<String>,
    pub(crate) exec: Option<String>,
    pub(crate) args: Option<Vec<String>>,
    pub(crate) sudo: bool,
    pub(crate) hook: PhaseHook,
}

#[derive(Debug, Default, PartialEq)]
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
                    .replace('\n', " ")
                    .replace('\r', " ")
                    .chars()
                    .take(50)
                    .collect::<String>()
            );
            info!("Executing `{}`", &shell_display);

            let shell_args = ["-c", shell];
            let output = temp_env::async_with_vars(
                [("DOD_CURRENT_MODULE", Some(module_path))],
                exec_output("sh", shell_args),
            )
            .await?;

            log_output!(output.stdout, "Stdout", &shell_display, info);
            log_output!(output.stderr, "Stderr", &shell_display, info);

            if output.status.success() {
                Ok(())
            } else {
                return Err(eyre!(
                    "Failed to execute {} from module {}",
                    shell,
                    &self.module_name
                ));
            }
        } else if let Some(ref exec) = self.exec {
            let args = self.args.as_deref().unwrap_or_else(|| &[]);
            let exec_display = format!("{} {}", exec, args.join(" "));

            if self.sudo {
                warn!("Executing `{}` with elevated permissions", exec_display);
            } else {
                info!("Executing `{}`", exec_display);
            }

            if self.sudo {
                let output = temp_env::async_with_vars(
                    [("DOD_CURRENT_MODULE", Some(module_path))],
                    pm.sudo_exec_output(
                        exec,
                        args,
                        Some(&format!(
                            "Executing {} with args: {}",
                            exec,
                            &self
                                .args
                                .as_ref()
                                .map(|a| a.join(" "))
                                .unwrap_or_else(|| "".to_string())
                        )),
                    ),
                )
                .await?;

                log_output!(output.stdout, "Stdout", exec_display, info);
                log_output!(output.stderr, "Stderr", exec_display, info);

                if output.status.success() {
                    Ok(())
                } else {
                    return Err(eyre!(
                        "Failed to execute {} from module {}",
                        exec,
                        &self.module_name
                    ));
                }
            } else {
                let output = temp_env::async_with_vars(
                    [("DOD_CURRENT_MODULE", Some(module_path))],
                    exec_output(exec, args),
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
