use std::ffi::OsStr;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};

// Adapted from https://github.com/Morganamilo/paru/blob/5355012aa3529014145b8940dd0c62b21e53095a/src/exec.rs#L144

/// Global variable, available to all threads, indicating if sudo is running.
static SUDO_LOOP_RUNNING: AtomicBool = AtomicBool::new(false);

/// Conditionally spawns a new thread to maintain an active `sudo` session by periodically
/// refreshing it.
///
/// If a `sudo` refresh loop is not already running (as indicated by [SUDO_LOOP_RUNNING]),
/// this function starts the loop. This ensures that subsequent operations requiring `sudo`
/// privileges can proceed without user interaction for entering passwords.
///
/// # Returns
///
/// * Returns `Ok(())` if the function successfully starts a `sudo` refresh loop or if one is
/// already running. Returns an `Err` if starting the `sudo` command fails at any point.
pub(crate) fn spawn_sudo_maybe<S: AsRef<str>>(reason: S) -> Result<()> {
    if crate::USE_SUDO.load(Ordering::Relaxed) {
        debug!("Requesting ROOT privileges. Reason: {}", reason.as_ref());
        if !SUDO_LOOP_RUNNING.load(Ordering::Relaxed) {
            SUDO_LOOP_RUNNING.store(true, Ordering::Relaxed);
            let sudo_cmd = "sudo".to_string();
            let flags = vec!["-v".to_string()];
            update_sudo(&sudo_cmd, &flags)?;
            thread::spawn(move || sudo_loop(&sudo_cmd, &flags));
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

/// Runs an infinite loop that periodically executes the `sudo` command with the specified flags to
/// keep the sudo session active. This loop runs in its own thread and sleeps for a specified
/// duration between `sudo` invocations.
///
/// # Arguments
///
/// * `sudo` - A string slice (`&str`) specifying the `sudo` command to execute.
/// * `flags` - A slice of elements that implement `AsRef<OsStr>`, representing additional flags or
/// arguments for the `sudo` command.
///
/// # Returns
///
/// *Returns `Ok(())` if the loop runs indefinitely without error. Returns an `Err` if executing the
/// `sudo` command fails.
fn sudo_loop<S: AsRef<OsStr>>(sudo: &str, flags: &[S]) -> Result<()> {
    loop {
        thread::sleep(Duration::from_secs(250));
        update_sudo(sudo, flags)?;
    }
}

/// Executes the `sudo` command with the specified flags once, inheriting the terminal's `stdin` to
/// allow for password input if necessary. This function is typically used to refresh the active
/// `sudo` session or check that `sudo` privileges can be obtained.
///
/// # Arguments
///
/// * `sudo` - A string slice (`&str`) specifying the `sudo` command to execute.
/// * `flags` - A slice of elements that implement `AsRef<OsStr>`, representing the flags or
/// arguments to be passed to the `sudo` command.
///
/// # Returns
///
/// * Returns `Ok(())` if the `sudo` command executes successfully. Returns an `Err` if the command
/// fails to execute or completes with a non-success status.
fn update_sudo<S: AsRef<OsStr>>(sudo: &str, flags: &[S]) -> Result<()> {
    let status = Command::new(sudo)
        .args(flags)
        // This allows sudo to read the password from the terminal
        .stdin(Stdio::inherit())
        .status()
        .with_context(|| "Failed to execute sudo command")?;

    if !status.success() {
        bail!("Sudo command failed");
    }
    Ok(())
}

/// Execute a command with sudo priviliges
pub(crate) async fn sudo_exec<S: AsRef<OsStr> + std::fmt::Debug>(
    cmd: &str,
    args: &[S],
    reason: Option<&str>,
) -> Result<()> {
    let reason = if let Some(reason) = reason {
        reason.to_string()
    } else {
        format!("Running {:?} with args: {:?}", cmd, args)
    };
    spawn_sudo_maybe(reason).context("Failed to spawn sudo")?;

    let mut cmd = tokio::process::Command::new("sudo")
        .arg(cmd)
        .args(args)
        .spawn()
        .with_context(|| format!("Failed to spawn {:?} with args: {:?}", cmd, args))?;

    if cmd.wait().await?.success() {
        Ok(())
    } else {
        bail!("Failed to execute {:?} with args: {:?}", cmd, args)
    }
}

/// Execute a command with sudo priviliges and return stdout
pub(crate) async fn sudo_exec_output<S: AsRef<OsStr> + std::fmt::Debug>(
    cmd: &str,
    args: &[S],
    reason: Option<&str>,
) -> Result<Vec<u8>> {
    let reason = if let Some(reason) = reason {
        reason.to_string()
    } else {
        format!("Running {:?} with args: {:?}", cmd, args)
    };
    spawn_sudo_maybe(reason).context("Failed to spawn sudo")?;

    let output = tokio::process::Command::new("sudo")
        .arg(cmd)
        .args(args)
        .output()
        .await
        .with_context(|| format!("Failed to execute {:?} with args: {:?}", cmd, args))?
        .stdout;

    Ok(output)
}

/// Execute a command with sudo priviliges and return true if exited succesfully.
pub(crate) async fn sudo_exec_success<S: AsRef<OsStr> + std::fmt::Debug>(
    cmd: &str,
    args: &[S],
    reason: Option<&str>,
) -> Result<bool> {
    let reason = if let Some(reason) = reason {
        reason.to_string()
    } else {
        format!("Running {:?} with args: {:?}", cmd, args)
    };
    spawn_sudo_maybe(reason).context("Failed to spawn sudo")?;

    let status = tokio::process::Command::new("sudo")
        .arg(cmd)
        .args(args)
        .status()
        .await
        .with_context(|| format!("Failed to execute {:?} with args: {:?}", cmd, args))?;

    Ok(status.success())
}
