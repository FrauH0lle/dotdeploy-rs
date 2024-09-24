//! Sudo operations module.
//!
//! This module provides functionality for executing commands with elevated privileges using sudo.
//! It includes mechanisms to maintain an active sudo session, execute commands with sudo, and
//! handle the output of sudo operations.
//!
//! The module is adapted from: https://github.com/Morganamilo/paru/blob/5355012aa3529014145b8940dd0c62b21e53095a/src/exec.rs#L144

use std::ffi::OsStr;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tokio::sync::Mutex;

use anyhow::{bail, Context, Result};
use lazy_static::lazy_static;

lazy_static! {
    /// Global variable, available to all threads, indicating if sudo is running.
    static ref SUDO_LOOP_RUNNING: AtomicBool = AtomicBool::new(false);
    /// Mutex for synchronizing access to the sudo session.
    static ref SUDO_MUTEX: Arc<Mutex<()>> = Arc::new(Mutex::new(()));
}

// NOTE 2024-09-24: If we ever want to support more than sudo, here would be the spot to implement
//   it.

#[derive(Debug, Clone)]
enum GetRootCmd {
    Sudo {
        cmd: String,
        initial_flags: Vec<String>,
        keepalive_flags: Vec<String>,
    },
}

impl GetRootCmd {
    fn use_sudo() -> Self {
        GetRootCmd::Sudo {
            cmd: "sudo".to_string(),
            initial_flags: vec!["-v".to_string()],
            keepalive_flags: vec!["-v".to_string(), "-n".to_string()],
        }
    }

    fn cmd(&self) -> &str {
        match self {
            GetRootCmd::Sudo { cmd, .. } => cmd,
        }
    }

    fn initial_flags(&self) -> &[String] {
        match self {
            GetRootCmd::Sudo { initial_flags, .. } => initial_flags,
        }
    }

    fn keepalive_flags(&self) -> &[String] {
        match self {
            GetRootCmd::Sudo {
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
pub(crate) async fn spawn_sudo_maybe<S: AsRef<str>>(reason: S) -> Result<()> {
    if crate::USE_SUDO.load(Ordering::Relaxed) {
        debug!("Requesting ROOT privileges. Reason: {}", reason.as_ref());
        let mut is_running = SUDO_LOOP_RUNNING.load(Ordering::Relaxed);
        if !is_running {
            let _guard = SUDO_MUTEX.lock().await;
            // Double-check the flag to handle race conditions
            is_running = SUDO_LOOP_RUNNING.load(Ordering::Relaxed);
            if !is_running {
                // Yield to allow any pending output to complete
                tokio::task::yield_now().await;

                // NOTE 2024-09-24: For the time being, we only support sudo and hardcode it here.
                let sudo_cmd = GetRootCmd::use_sudo();

                // HACK 2024-09-24: I don't know so far how to fix the issue that the password
                //   prompt gets interleaved into the programm output. This is a stupid hack but
                //   works most of the time.
                //
                // FIXME 2024-09-24: There should be a cleaner and better solution to this.
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

                std::thread::spawn(move || sudo_loop(&sudo_cmd));
                SUDO_LOOP_RUNNING.store(true, Ordering::Relaxed);
            }
        } else {
            debug!("sudo loop already running.")
        }
        Ok(())
    } else {
        bail!(
            "Use of 'sudo' is disabled.
Check the value of the variable `use_sudo` in `$HOME/.config/dotdeploy/config.toml`"
        )
    }
}

/// Runs an infinite loop that periodically executes the `sudo` command to keep the sudo session
/// active.
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
fn sudo_loop(sudo: &GetRootCmd) -> Result<()> {
    debug!("Executing privilege escalation command");
    let status = Command::new(sudo.cmd())
        .args(sudo.initial_flags())
        .status()
        .with_context(|| "Failed to execute sudo command")?;

    if !status.success() {
        bail!("Sudo command failed");
    }

    debug!("Running sudo loop");
    loop {
        update_sudo(sudo)?;
        thread::sleep(Duration::from_secs(60));
    }
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
fn update_sudo(sudo: &GetRootCmd) -> Result<()> {
    debug!("Updating privilege escalation");
    let status = Command::new(sudo.cmd())
        .args(sudo.keepalive_flags())
        .status()
        .with_context(|| "Failed to execute sudo command")?;

    if !status.success() {
        bail!("Sudo command failed");
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
        .context("Failed to spawn sudo")?;

    let mut exec = tokio::process::Command::new("sudo")
        .arg(cmd)
        .args(args)
        .spawn()
        .with_context(|| format!("Failed to execute sudo {} {}", cmd, format_args(args)))?;

    if exec.wait().await?.success() {
        Ok(())
    } else {
        bail!("Failed to execute sudo {} {}", cmd, format_args(args))
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
        .context("Failed to spawn sudo")?;

    let output = tokio::process::Command::new("sudo")
        .arg(cmd)
        .args(args)
        .output()
        .await
        .with_context(|| format!("Failed to execute sudo {} {}", cmd, format_args(args)))?;

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
        .context("Failed to spawn sudo")?;

    let status = tokio::process::Command::new("sudo")
        .arg(cmd)
        .args(args)
        .status()
        .await
        .with_context(|| format!("Failed to execute sudo {} {}", cmd, format_args(args)))?;

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
        assert!(sudo_exec("test", &["4", "-gt", "0"], None).await.is_ok());
        assert!(sudo_exec("test", &["4", "-eq", "0"], None).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_sudo_exec_output() -> Result<()> {
        crate::USE_SUDO.store(true, std::sync::atomic::Ordering::Relaxed);
        let output = sudo_exec_output("echo", &["-n", "success"], None).await?;
        assert!(!output.stdout.is_empty());
        let output = sudo_exec_output("echo", &["-n"], None).await?;
        assert!(output.stdout.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_sudo_exec_success() -> Result<()> {
        crate::USE_SUDO.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(sudo_exec_success("test", &["4", "-gt", "0"], None).await?);
        assert!(!sudo_exec_success("test", &["4", "-eq", "0"], None).await?);
        Ok(())
    }
}
