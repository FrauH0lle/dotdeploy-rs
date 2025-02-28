//! Sudo operations module.
//!
//! This module provides functionality for executing commands with elevated privileges using sudo.
//! It includes mechanisms to maintain an active sudo session, execute commands with sudo, and
//! handle the output of sudo operations.
//!
//! The module is adapted from: <https://github.com/Morganamilo/paru/blob/5355012aa3529014145b8940dd0c62b21e53095a/src/exec.rs#L144>

use crate::utils::commands::exec_output;
use color_eyre::eyre::{OptionExt, WrapErr, eyre};
use color_eyre::{Result, Section};
use std::ffi::{OsStr, OsString};
use std::process::{Command, ExitStatus, Output};
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::debug;

// -------------------------------------------------------------------------------------------------
//  PrivilegeManager
// -------------------------------------------------------------------------------------------------

#[derive(Debug)]
pub(crate) struct PrivilegeManager {
    /// Flag indicating whether sudo/privilege elevation should be used.
    ///
    /// This is set during initialization based on configuration and used throughout the application
    /// to determine if operations need elevated privileges.
    use_sudo: bool,
    /// Storage for the sudo command to be used for privilege elevation.
    ///
    /// This is initialized during startup based on configuration and stores the specific command
    /// (e.g., "sudo", "doas") that should be used when elevated privileges are needed.
    pub(crate) root_cmd: GetRootCmd,
    /// Lock to synchronize terminal access for privilege elevation prompts.
    ///
    /// This ensures that sudo password prompts and similar terminal interactions don't overlap and
    /// confuse the user, especially in concurrent operations.
    terminal_lock: Arc<RwLock<()>>,
    /// Mutex to synchronize access to sudo operations.
    ///
    /// This ensures that sudo password prompts and privilege elevation operations don't interfere
    /// with each other when running concurrently.
    sudo_lock: Arc<Mutex<()>>,
    /// Flag indicating whether a sudo refresh loop is currently running.
    ///
    /// This prevents multiple sudo refresh loops from being started simultaneously.
    pub(crate) loop_running: AtomicBool,
    channel_rx: Mutex<Option<mpsc::Receiver<()>>>,
}

// --
// * PrivilegeManager Builder

/// Builder for [`PrivilegeManager`]. See its documentation for details.
#[derive(Debug, Default)]
pub(crate) struct PrivilegeManagerBuilder {
    use_sudo: Option<bool>,
    root_cmd: Option<GetRootCmd>,
    terminal_lock: Option<Arc<RwLock<()>>>,
    channel_rx: Option<mpsc::Receiver<()>>,
}

impl PrivilegeManagerBuilder {
    /// Creates a new PrivilegeManagerBuilder with default settings.
    pub(crate) fn new() -> Self {
        PrivilegeManagerBuilder::default()
    }

    /// Sets the availability of privilege elevation.
    ///
    /// # Arguments
    /// * `use_sudo` - Whether privilege elevation is available
    pub(crate) fn with_use_sudo(&mut self, use_sudo: bool) -> &mut Self {
        let new = self;
        new.use_sudo = Some(use_sudo);
        new
    }

    /// Sets the privilege elevation command to be used.
    ///
    /// # Arguments
    /// * `root_cmd` - The root command variant to use (sudo/doas configuration)
    pub(crate) fn with_root_cmd(&mut self, root_cmd: GetRootCmd) -> &mut Self {
        let new = self;
        new.root_cmd = Some(root_cmd);
        new
    }

    /// Sets the terminal synchronization lock for privilege elevation prompts.
    ///
    /// # Arguments
    /// * `terminal_lock` - Shared RwLock for coordinating terminal access
    ///
    /// This should be shared with the logger's terminal lock to ensure consistent output
    /// coordination between logging and privilege elevation operations.
    pub(crate) fn with_terminal_lock(&mut self, terminal_lock: Arc<RwLock<()>>) -> &mut Self {
        let new = self;
        new.terminal_lock = Some(terminal_lock);
        new
    }

    pub(crate) fn with_channel_rx(&mut self, channel_rx: Option<mpsc::Receiver<()>>) -> &mut Self {
        let new = self;
        new.channel_rx = channel_rx;
        new
    }

    /// Constructs the PrivilegeManager instance after validating all required fields.
    ///
    /// # Errors
    /// Returns an error if any required fields are missing.
    pub(crate) fn build(&mut self) -> Result<PrivilegeManager> {
        Ok(PrivilegeManager {
            use_sudo: Clone::clone(
                self.use_sudo
                    .as_ref()
                    .ok_or_eyre("Empty 'use_sudo' field")?,
            ),
            root_cmd: Clone::clone(
                self.root_cmd
                    .as_ref()
                    .ok_or_eyre("Empty 'root_cmd' field")?,
            ),
            terminal_lock: Clone::clone(
                self.terminal_lock
                    .as_ref()
                    .ok_or_eyre("Empty 'terminal_lock' field")?,
            ),
            sudo_lock: Arc::new(Mutex::new(())),
            loop_running: AtomicBool::new(false),
            channel_rx: match self.channel_rx.is_some() {
                true => Mutex::new(self.channel_rx.take()),
                false => return Err(eyre!("Empty 'channel_rx' field")),
            },
        })
    }
}

// -------------------------------------------------------------------------------------------------
// Root command enum
// -------------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) enum GetRootCmd {
    Sudo {
        cmd: OsString,
        initial_flags: Vec<OsString>,
        keepalive_flags: Vec<OsString>,
        termination_flags: Vec<OsString>,
    },
    Doas {
        cmd: OsString,
        initial_flags: Vec<OsString>,
        keepalive_flags: Vec<OsString>,
        termination_flags: Vec<OsString>,
    },
}

impl GetRootCmd {
    /// Creates a new GetRootCmd instance configured for sudo usage.
    pub(crate) fn use_sudo() -> Self {
        GetRootCmd::Sudo {
            cmd: OsString::from("sudo"),
            initial_flags: vec![OsString::from("-v")],
            keepalive_flags: vec![OsString::from("-v"), OsString::from("-n")],
            termination_flags: vec![OsString::from("-k")],
        }
    }

    /// Creates a new GetRootCmd instance configured for doas usage.
    pub(crate) fn use_doas() -> Self {
        GetRootCmd::Doas {
            cmd: OsString::from("doas"),
            initial_flags: vec![],
            keepalive_flags: vec![OsString::from("-n")],
            termination_flags: vec![OsString::from("-L")],
        }
    }

    /// Returns the command string for privilege elevation.
    ///
    /// For sudo, this returns "sudo".
    /// For doas, this returns "doas".
    pub(crate) fn cmd(&self) -> &OsString {
        match self {
            GetRootCmd::Sudo { cmd, .. } => cmd,
            GetRootCmd::Doas { cmd, .. } => cmd,
        }
    }

    /// Returns the flags used for initial authentication.
    ///
    /// For sudo, this returns ["-v"] to validate credentials.
    /// For doas, this returns [] to validate credentials.
    fn initial_flags(&self) -> &[OsString] {
        match self {
            GetRootCmd::Sudo { initial_flags, .. } => initial_flags,
            GetRootCmd::Doas { initial_flags, .. } => initial_flags,
        }
    }

    /// Returns the flags used to keep the session alive.
    ///
    /// For sudo, this returns ["-v", "-n"] to validate credentials without prompt.
    /// For doas, this returns ["-n"] to validate credentials without prompt.
    fn keepalive_flags(&self) -> &[OsString] {
        match self {
            GetRootCmd::Sudo {
                keepalive_flags, ..
            } => keepalive_flags,
            GetRootCmd::Doas {
                keepalive_flags, ..
            } => keepalive_flags,
        }
    }

    /// Returns the flags used to keep the session alive.
    ///
    /// For sudo, this returns ["-k"] to clear persisted authentications.
    /// For doas, this returns ["-L"] to clear persisted authentications.
    fn termination_flags(&self) -> &[OsString] {
        match self {
            GetRootCmd::Sudo {
                termination_flags, ..
            } => termination_flags,
            GetRootCmd::Doas {
                termination_flags, ..
            } => termination_flags,
        }
    }
}

impl PrivilegeManager {
    /// Conditionally spawns a new thread to maintain an active `sudo` session by periodically
    /// refreshing it.
    ///
    /// If a `sudo` refresh loop is not already running (as indicated by
    /// [`field@PrivilegeManager::loop_running`]), this function starts the loop. This ensures that
    /// subsequent operations requiring `sudo` privileges can proceed without user interaction for
    /// entering passwords.
    ///
    /// # Arguments
    ///
    /// * `reason` - A string slice explaining the reason for requesting sudo privileges.
    ///
    /// # Errors
    ///
    /// Returns an error if starting the sudo command fails or if sudo use is disabled.
    pub(crate) async fn spawn_sudo_maybe<S: AsRef<str>>(&self, reason: S) -> Result<()> {
        if self.use_sudo {
            debug!(reason = reason.as_ref(), "Requesting ROOT privileges");
            let mut is_running = self.loop_running.load(Ordering::Relaxed);
            if !is_running {
                let mut _guard = self.sudo_lock.lock().await;
                // Double-check the flag to handle race conditions
                is_running = self.loop_running.load(Ordering::Relaxed);
                if !is_running {
                    // Run sudo loop
                    debug!(
                        cmd = format!(
                            "{} {}",
                            self.root_cmd.cmd().to_string_lossy(),
                            format_args(self.root_cmd.initial_flags())
                        ),
                        "Executing privilege elevation command"
                    );

                    let guard = self.terminal_lock.write();
                    let status = Command::new(self.root_cmd.cmd())
                        .args(self.root_cmd.initial_flags())
                        .status()
                        .wrap_err("Failed to execute privilege elevation command")?;

                    if !status.success() {
                        return Err(eyre!("Privilege elevation command failed"));
                    }

                    drop(guard);

                    debug!("Running privilege refresh loop");
                    let sudo_clone = self.root_cmd.clone();
                    let rx = {
                        let mut channel_guard = self.channel_rx.lock().await;
                        channel_guard.take().unwrap()
                    };
                    let _handle = thread::spawn(move || {
                        let mut counter = 0;
                        loop {
                            counter += 1;

                            match rx.try_recv() {
                                Ok(_) | Err(TryRecvError::Disconnected) => {
                                    debug!(
                                        cmd = format!(
                                            "{} {}",
                                            sudo_clone.cmd().to_string_lossy(),
                                            format_args(sudo_clone.termination_flags())
                                        ),
                                        "Terminating privilege refresh loop"
                                    );
                                    let status = std::process::Command::new(sudo_clone.cmd())
                                        .args(sudo_clone.termination_flags())
                                        .status()
                                        .expect("Failed to run privilege termination command");
                                    if status.success() {
                                        break;
                                    } else {
                                        panic!(
                                            "Failed to terminate privilege refresh loop! {status}"
                                        )
                                    }
                                }
                                Err(TryRecvError::Empty) => {}
                            }

                            if counter == 120 {
                                let _ = update_sudo(&sudo_clone);
                                debug!(
                                    cmd = format!(
                                        "{} {}",
                                        sudo_clone.cmd().to_string_lossy(),
                                        format_args(sudo_clone.keepalive_flags())
                                    ),
                                    "Privileges updated"
                                );
                                counter = 0;
                            }
                            thread::sleep(Duration::from_millis(500));
                        }
                    });
                    self.loop_running.store(true, Ordering::Relaxed);
                }
            } else {
                debug!("Privilege refresh loop already running.")
            }
            Ok(())
        } else {
            return Err(eyre!(
                "Use of '{}' is disabled. ",
                self.root_cmd.cmd().to_string_lossy()
            )
            .suggestion("Check the value of the variable `use_sudo` in your config file"));
        }
    }

    /// Executes a command with sudo privileges.
    ///
    /// # Arguments
    ///
    /// * `cmd` - The command to execute.
    /// * `args` - Arguments for the command.
    /// * `reason` - Optional reason for sudo execution, used for logging.
    ///
    /// # Errors
    ///
    /// Returns an error if the command fails to execute or returns a non-zero exit status.
    pub(crate) async fn sudo_exec<S, I>(&self, cmd: S, args: I, reason: Option<&str>) -> Result<()>
    where
        S: AsRef<OsStr>,
        I: IntoIterator<Item = S>,
    {
        let exec = self.sudo_exec_1(cmd, args, reason).await?;
        if exec.success() {
            Ok(())
        } else {
            Err(eyre!("Elevated execution failed"))
        }
    }

    async fn sudo_exec_1<S, I>(&self, cmd: S, args: I, reason: Option<&str>) -> Result<ExitStatus>
    where
        S: AsRef<OsStr>,
        I: IntoIterator<Item = S>,
    {
        let args_os: Vec<OsString> = args
            .into_iter()
            .map(|a| a.as_ref().to_os_string())
            .collect();

        let reason = if let Some(reason) = reason {
            reason.to_string()
        } else {
            format!(
                "Executing: {} {} {}",
                self.root_cmd.cmd().to_string_lossy(),
                cmd.as_ref().to_string_lossy(),
                format_args(&args_os)
            )
        };

        self.spawn_sudo_maybe(reason).await.wrap_err_with(|| {
            format!("Failed to spawn {}", self.root_cmd.cmd().to_string_lossy())
        })?;

        let mut exec = tokio::process::Command::new(self.root_cmd.cmd())
            .arg(&cmd)
            .args(&args_os)
            .spawn()
            .wrap_err_with(|| {
                format!(
                    "Failed to execute {} {} {}",
                    self.root_cmd.cmd().to_string_lossy(),
                    cmd.as_ref().to_string_lossy(),
                    format_args(&args_os)
                )
            })?;

        let result = exec.wait().await?;
        Ok(result)
    }

    /// Executes a command with sudo privileges and returns its output.
    ///
    /// # Arguments
    ///
    /// * `cmd` - The command to execute.
    /// * `args` - Arguments for the command.
    /// * `reason` - Optional reason for sudo execution, used for logging.
    ///
    /// # Errors
    /// Returns an error if the command fails to execute.
    pub(crate) async fn sudo_exec_output<S, I>(
        &self,
        cmd: S,
        args: I,
        reason: Option<&str>,
    ) -> Result<Output>
    where
        S: AsRef<OsStr>,
        I: IntoIterator<Item = S>,
    {
        let args_os: Vec<OsString> = args
            .into_iter()
            .map(|a| a.as_ref().to_os_string())
            .collect();

        let reason = if let Some(reason) = reason {
            reason.to_string()
        } else {
            format!(
                "Executing: sudo {} {}",
                cmd.as_ref().to_string_lossy(),
                format_args(&args_os)
            )
        };
        self.spawn_sudo_maybe(reason)
            .await
            .wrap_err("Failed to spawn sudo")?;

        exec_output(
            self.root_cmd.cmd(),
            &[vec![cmd.as_ref().to_os_string()], args_os].concat(),
        )
        .await
    }

    /// Executes a command with sudo privileges and returns whether it exited successfully.
    ///
    /// # Arguments
    ///
    /// * `cmd` - The command to execute.
    /// * `args` - Arguments for the command.
    /// * `reason` - Optional reason for sudo execution, used for logging.
    ///
    /// # Errors
    /// Returns an error if the command fails to execute.
    pub(crate) async fn sudo_exec_success<S, I>(
        &self,
        cmd: S,
        args: I,
        reason: Option<&str>,
    ) -> Result<bool>
    where
        S: AsRef<OsStr>,
        I: IntoIterator<Item = S>,
    {
        let exec = self.sudo_exec_1(cmd, args, reason).await?;
        Ok(exec.success())
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
/// # Errors
///
/// Returns an error if the command fails to execute or completes with a non-success status.
fn update_sudo(sudo: &GetRootCmd) -> Result<()> {
    debug!(
        cmd = format!(
            "{} {}",
            &sudo.cmd().to_string_lossy(),
            format_args(sudo.keepalive_flags())
        ),
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
fn format_args<I, S>(args: I) -> String
where
    S: AsRef<OsStr>,
    I: IntoIterator<Item = S>,
{
    args.into_iter()
        .map(|a| a.as_ref().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

// -------------------------------------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests;

    #[tokio::test]
    async fn test_sudo_exec() -> Result<()> {
        let (_tx, pm) = tests::pm_setup()?;

        assert!(pm.sudo_exec("test", ["4", "-gt", "0"], None).await.is_ok());
        assert!(pm.sudo_exec("test", ["4", "-eq", "0"], None).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_sudo_exec_output() -> Result<()> {
        let (_tx, pm) = tests::pm_setup()?;

        let output = pm.sudo_exec_output("echo", ["-n", "success"], None).await?;
        assert!(!output.stdout.is_empty());
        let output = pm.sudo_exec_output("echo", ["-n"], None).await?;
        assert!(output.stdout.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_sudo_exec_success() -> Result<()> {
        let (_tx, pm) = tests::pm_setup()?;

        assert!(
            pm.sudo_exec_success("test", ["4", "-gt", "0"], None)
                .await?
        );
        assert!(
            !pm.sudo_exec_success("test", ["4", "-eq", "0"], None)
                .await?
        );
        Ok(())
    }
}
