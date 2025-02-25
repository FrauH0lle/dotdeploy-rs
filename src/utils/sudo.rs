//! Sudo operations module.
//!
//! This module provides functionality for executing commands with elevated privileges using sudo.
//! It includes mechanisms to maintain an active sudo session, execute commands with sudo, and
//! handle the output of sudo operations.
//!
//! The module is adapted from: <https://github.com/Morganamilo/paru/blob/5355012aa3529014145b8940dd0c62b21e53095a/src/exec.rs#L144>

use crate::{SUDO_CMD, TERMINAL_LOCK};
use color_eyre::eyre::{OptionExt, WrapErr, eyre};
use color_eyre::{Result, Section};
use std::ffi::OsStr;
use std::process::Command;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{debug, instrument};

#[derive(Debug)]
pub(crate) struct PrivilegeManager {
    use_sudo: bool,
    root_cmd: GetRootCmd,
    terminal_lock: Arc<RwLock<()>>,
    sudo_lock: Arc<Mutex<()>>,
    loop_running: bool
}

#[derive(Debug, Default)]
pub(crate) struct PrivilegeManagerBuilder {
    use_sudo: Option<bool>,
    root_cmd: Option<GetRootCmd>,
    terminal_lock: Option<Arc<RwLock<()>>>,
}

impl PrivilegeManagerBuilder {
    pub(crate) fn new() -> Self {
        PrivilegeManagerBuilder::default()
    }

    pub(crate) fn with_use_sudo(&mut self, use_sudo: bool) -> &mut Self {
        let new = self;
        new.use_sudo = Some(use_sudo);
        new
    }

    pub(crate) fn with_root_cmd(&mut self, root_cmd: GetRootCmd) -> &mut Self {
        let new = self;
        new.root_cmd = Some(root_cmd);
        new
    }

    pub(crate) fn with_terminal_lock(&mut self, terminal_lock: Arc<RwLock<()>>) -> &mut Self {
        let new = self;
        new.terminal_lock = Some(terminal_lock);
        new
    }

    pub(crate) fn build(&self) -> Result<PrivilegeManager> {
        Ok(PrivilegeManager {
            use_sudo: Clone::clone(self.use_sudo.as_ref().ok_or_eyre("Empty builder field")?),
            root_cmd: Clone::clone(self.root_cmd.as_ref().ok_or_eyre("Empty builder field")?),
            terminal_lock: Clone::clone(
                self.terminal_lock
                    .as_ref()
                    .ok_or_eyre("Empty builder field")?,
            ),
        })
    }
}

// -------------------------------------------------------------------------------------------------
// Global Variables
// -------------------------------------------------------------------------------------------------

/// Global flag indicating whether a sudo refresh loop is currently running.
///
/// This prevents multiple sudo refresh loops from being started simultaneously.
static SUDO_LOOP_RUNNING: LazyLock<AtomicBool> = LazyLock::new(|| AtomicBool::new(false));

/// Global mutex to synchronize access to sudo operations.
///
/// This ensures that sudo password prompts and privilege elevation operations don't interfere with
/// each other when running concurrently.
static SUDO_MUTEX: LazyLock<Arc<Mutex<()>>> = LazyLock::new(|| Arc::new(Mutex::new(())));

// NOTE 2024-09-24: If we ever want to support more than sudo, here would be the spot to implement
//   it.

#[derive(Debug, Clone)]
pub(crate) enum GetRootCmd {
    Sudo {
        cmd: String,
        initial_flags: Vec<String>,
        keepalive_flags: Vec<String>,
    },
    Doas {
        cmd: String,
        initial_flags: Vec<String>,
        keepalive_flags: Vec<String>,
    },
}

impl GetRootCmd {
    /// Creates a new GetRootCmd instance configured for sudo usage.
    pub(crate) fn use_sudo() -> Self {
        GetRootCmd::Sudo {
            cmd: "sudo".to_string(),
            initial_flags: vec!["-v".to_string()],
            keepalive_flags: vec!["-v".to_string(), "-n".to_string()],
        }
    }

    /// Creates a new GetRootCmd instance configured for doas usage.
    pub(crate) fn use_doas() -> Self {
        GetRootCmd::Doas {
            cmd: "doas".to_string(),
            initial_flags: vec![],
            keepalive_flags: vec!["-n".to_string()],
        }
    }

    /// Returns the command string for privilege elevation.
    ///
    /// For sudo, this returns "sudo".
    fn cmd(&self) -> &str {
        match self {
            GetRootCmd::Sudo { cmd, .. } => cmd,
            GetRootCmd::Doas { cmd, .. } => cmd,
        }
    }

    /// Returns the flags used for initial authentication.
    ///
    /// For sudo, this returns ["-v"] to validate credentials.
    fn initial_flags(&self) -> &[String] {
        match self {
            GetRootCmd::Sudo { initial_flags, .. } => initial_flags,
            GetRootCmd::Doas { initial_flags, .. } => initial_flags,
        }
    }

    /// Returns the flags used to keep the session alive.
    ///
    /// For sudo, this returns ["-v", "-n"] to validate credentials without prompt.
    fn keepalive_flags(&self) -> &[String] {
        match self {
            GetRootCmd::Sudo {
                keepalive_flags, ..
            } => keepalive_flags,
            GetRootCmd::Doas {
                keepalive_flags, ..
            } => keepalive_flags,
        }
    }
}

/// Conditionally spawns a new thread to maintain an active `sudo` session by periodically
/// refreshing it.
///
/// If a `sudo` refresh loop is not already running (as indicated by [SUDO_LOOP_RUNNING]), this
/// function starts the loop. This ensures that subsequent operations requiring `sudo` privileges
/// can proceed without user interaction for entering passwords.
///
/// # Arguments
///
/// * `reason` - A string slice explaining the reason for requesting sudo privileges.
///
/// # Returns
///
/// * `Ok(())` if the function successfully starts a `sudo` refresh loop or if one is
///   already running.
/// * `Err` if starting the `sudo` command fails or if sudo use is disabled.
#[instrument]
pub(crate) async fn spawn_sudo_maybe<S: AsRef<str> + std::fmt::Debug>(reason: S) -> Result<()> {
    if crate::USE_SUDO.load(Ordering::Relaxed) {
        debug!(reason = reason.as_ref(), "Requesting ROOT privileges");
        let mut is_running = SUDO_LOOP_RUNNING.load(Ordering::Relaxed);
        if !is_running {
            let _guard = SUDO_MUTEX.lock().await;
            // Double-check the flag to handle race conditions
            is_running = SUDO_LOOP_RUNNING.load(Ordering::Relaxed);
            if !is_running {
                let sudo_cmd = match SUDO_CMD.get() {
                    Some(cmd) if cmd == "sudo" => GetRootCmd::use_sudo(),
                    Some(_) => return Err(eyre!("Unknown 'sudo' command")),
                    None => {
                        return Err(eyre!("'sudo' command not set")
                            .suggestion("Check the value of 'sudo_cmd' in the dotdeploy config"));
                    }
                };

                // Run sudo loop
                sudo_loop(&sudo_cmd)?;
                SUDO_LOOP_RUNNING.store(true, Ordering::Relaxed);
            }
        } else {
            debug!("sudo loop already running.")
        }
        Ok(())
    } else {
        return Err(eyre!(
            "Use of 'sudo' is disabled.
Check the value of the variable `use_sudo` in `$HOME/.config/dotdeploy/config.toml`"
        ));
    }
}

/// Runs an infinite loop in separate thread that periodically executes the `sudo` command to keep
/// the sudo session active.
///
/// This function is intended to be run in its own thread. It sleeps for a specified duration
/// between `sudo` invocations to avoid unnecessary system load.
///
/// # Arguments
///
/// * `sudo` - The sudo command variant to execute.
///
/// # Returns
///
/// * `Ok(())` if the loop runs indefinitely without error.
/// * `Err` if executing the sudo command fails.
#[instrument]
fn sudo_loop(sudo: &GetRootCmd) -> Result<()> {
    debug!(
        cmd = format!("{} {}", &sudo.cmd(), format_args(sudo.initial_flags())),
        "Executing privilege elevation command"
    );

    let guard = TERMINAL_LOCK.write();
    let status = Command::new(sudo.cmd())
        .args(sudo.initial_flags())
        .status()
        .wrap_err("Failed to execute sudo command")?;

    if !status.success() {
        return Err(eyre!("Sudo command failed"));
    }

    drop(guard);

    debug!("Running sudo loop");
    let sudo_clone = sudo.clone();
    let _handle = std::thread::spawn(move || {
        loop {
            let _ = update_sudo(&sudo_clone);
            debug!("Privileges updated");
            thread::sleep(Duration::from_secs(60));
        }
    });
    Ok(())
}

/// Executes the `sudo` command with the specified flags once.
///
/// This function is typically used to refresh the active sudo session or check that sudo privileges
/// can be obtained.
///
/// # Arguments
///
/// * `sudo` - The sudo command variant to execute.
///
/// # Returns
///
/// * `Ok(())` if the sudo command executes successfully.
/// * `Err` if the command fails to execute or completes with a non-success status.
#[instrument]
fn update_sudo(sudo: &GetRootCmd) -> Result<()> {
    debug!(
        cmd = format!("{} {}", &sudo.cmd(), format_args(sudo.keepalive_flags())),
        "Updating privileges"
    );
    let status = Command::new(sudo.cmd())
        .args(sudo.keepalive_flags())
        .status()
        .wrap_err("Failed to execute sudo command")?;

    if !status.success() {
        return Err(eyre!("Sudo command failed"));
    }
    Ok(())
}

/// Formats the given arguments into a single string for printing.
///
/// # Arguments
///
/// * `args` - A slice of arguments to format.
///
/// # Returns
///
/// A string containing all arguments joined with spaces.
fn format_args<S: AsRef<OsStr>>(args: &[S]) -> String {
    args.iter()
        .map(|s| s.as_ref().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Executes a command with sudo privileges.
///
/// # Arguments
///
/// * `cmd` - The command to execute.
/// * `args` - Arguments for the command.
/// * `reason` - Optional reason for sudo execution, used for logging.
///
/// # Returns
///
/// * `Ok(())` if the command executes successfully.
/// * `Err` if the command fails to execute or returns a non-zero exit status.
pub(crate) async fn sudo_exec<S: AsRef<OsStr>>(
    cmd: &str,
    args: &[S],
    reason: Option<&str>,
) -> Result<()> {
    let reason = if let Some(reason) = reason {
        reason.to_string()
    } else {
        format!("Executing: sudo {} {}", cmd, format_args(args))
    };
    spawn_sudo_maybe(reason)
        .await
        .wrap_err("Failed to spawn sudo")?;

    let mut exec = tokio::process::Command::new("sudo")
        .arg(cmd)
        .args(args)
        .spawn()
        .wrap_err_with(|| format!("Failed to execute sudo {} {}", cmd, format_args(args)))?;

    if exec.wait().await?.success() {
        Ok(())
    } else {
        Err(eyre!(
            "Failed to execute sudo {} {}",
            cmd,
            format_args(args)
        ))
    }
}

/// Executes a command with sudo privileges and returns its output.
///
/// # Arguments
///
/// * `cmd` - The command to execute.
/// * `args` - Arguments for the command.
/// * `reason` - Optional reason for sudo execution, used for logging.
///
/// # Returns
///
/// * `Ok(Output)` containing the command's output if it executes successfully.
/// * `Err` if the command fails to execute.
pub(crate) async fn sudo_exec_output<S: AsRef<OsStr>>(
    cmd: &str,
    args: &[S],
    reason: Option<&str>,
) -> Result<std::process::Output> {
    let reason = if let Some(reason) = reason {
        reason.to_string()
    } else {
        format!("Executing: sudo {} {}", cmd, format_args(args))
    };
    spawn_sudo_maybe(reason)
        .await
        .wrap_err("Failed to spawn sudo")?;

    let output = tokio::process::Command::new("sudo")
        .arg(cmd)
        .args(args)
        .output()
        .await
        .wrap_err_with(|| format!("Failed to execute sudo {} {}", cmd, format_args(args)))?;

    Ok(output)
}

/// Executes a command with sudo privileges and returns whether it exited successfully.
///
/// # Arguments
///
/// * `cmd` - The command to execute.
/// * `args` - Arguments for the command.
/// * `reason` - Optional reason for sudo execution, used for logging.
///
/// # Returns
///
/// * `Ok(bool)` indicating whether the command executed successfully.
/// * `Err` if the command fails to execute.
pub(crate) async fn sudo_exec_success<S: AsRef<OsStr>>(
    cmd: &str,
    args: &[S],
    reason: Option<&str>,
) -> Result<bool> {
    let reason = if let Some(reason) = reason {
        reason.to_string()
    } else {
        format!("Executing: sudo {} {}", cmd, format_args(args))
    };
    spawn_sudo_maybe(reason)
        .await
        .wrap_err("Failed to spawn sudo")?;

    let status = tokio::process::Command::new("sudo")
        .arg(cmd)
        .args(args)
        .status()
        .await
        .wrap_err_with(|| format!("Failed to execute sudo {} {}", cmd, format_args(args)))?;

    Ok(status.success())
}

//
// Tests

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sudo_exec() -> Result<()> {
        crate::USE_SUDO.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = crate::SUDO_CMD.set("sudo".to_string());

        assert!(sudo_exec("test", &["4", "-gt", "0"], None).await.is_ok());
        assert!(sudo_exec("test", &["4", "-eq", "0"], None).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_sudo_exec_output() -> Result<()> {
        crate::USE_SUDO.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = crate::SUDO_CMD.set("sudo".to_string());

        let output = sudo_exec_output("echo", &["-n", "success"], None).await?;
        assert!(!output.stdout.is_empty());
        let output = sudo_exec_output("echo", &["-n"], None).await?;
        assert!(output.stdout.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_sudo_exec_success() -> Result<()> {
        crate::USE_SUDO.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = crate::SUDO_CMD.set("sudo".to_string());

        assert!(sudo_exec_success("test", &["4", "-gt", "0"], None).await?);
        assert!(!sudo_exec_success("test", &["4", "-eq", "0"], None).await?);
        Ok(())
    }
}
