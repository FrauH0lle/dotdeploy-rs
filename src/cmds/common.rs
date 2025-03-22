use crate::logs::log_output;
use crate::utils::commands::exec_output;
use crate::utils::sudo::PrivilegeManager;
use color_eyre::eyre::eyre;
use color_eyre::Result;
use std::ffi::{OsStr, OsString};
use tracing::info;

pub(crate) async fn exec_package_cmd<S, I>(cmd: I, packages: I, pm: &PrivilegeManager) -> Result<()>
where
    S: AsRef<OsStr>,
    I: IntoIterator<Item = S>,
{
    let cmd = cmd
        .into_iter()
        .map(|x| x.as_ref().to_os_string())
        .collect::<Vec<OsString>>();
    let first_cmd = &cmd[0];
    let packages = packages
        .into_iter()
        .map(|x| x.as_ref().to_os_string())
        .collect::<Vec<OsString>>();

    let output = if first_cmd == pm.root_cmd.cmd() {
        pm.sudo_exec_output(&cmd[1], &[&cmd[2..], &packages].concat(), None)
            .await?
    } else {
        exec_output(&cmd[0], &[&cmd[1..], &packages].concat()).await?
    };

    let cmd_str = cmd
        .iter()
        .map(|a| a.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    log_output!(
        output.stdout,
        "Stdout",
        format!("{} ...", cmd_str.join(" ")),
        info
    );
    log_output!(
        output.stderr,
        "Stderr",
        format!("{} ...", cmd_str.join(" ")),
        info
    );

    if !output.status.success() {
        return Err(eyre!("Failed to install packages"));
    }

    Ok(())
}
