use crate::logs::log_output;
use crate::utils::commands::exec_output;
use crate::utils::sudo::PrivilegeManager;
use color_eyre::Result;
use color_eyre::eyre::eyre;
use std::ffi::{OsStr, OsString};
use tracing::info;

/// Executes package management commands with privilege escalation handling
///
/// Safely runs package operations (install/remove/etc.) with automatic sudo handling based on
/// system configuration. Formats command output according to the application's logging standards.
///
/// # Arguments
/// * `command_parts` - Base command components (e.g. ["apt", "install"])
/// * `packages` - Package names to operate on
/// * `pm` - Privilege manager for handling root elevation
///
/// # Errors
/// Returns an error if:
/// - Command execution fails (non-zero exit status)
/// - I/O operations during command spawning fail
/// - Privilege escalation is required but unavailable
pub(crate) async fn exec_package_cmd<S, I>(
    command_parts: I,
    packages: I,
    pm: &PrivilegeManager,
) -> Result<()>
where
    S: AsRef<OsStr>,
    I: IntoIterator<Item = S>,
{
    // Convert command parts to OsString for cross-platform compatibility
    let cmd_args: Vec<OsString> = command_parts
        .into_iter()
        .map(|part| part.as_ref().to_os_string())
        .collect();

    // Convert package list to OsString while preserving original ordering
    let package_args: Vec<OsString> = packages
        .into_iter()
        .map(|pkg| pkg.as_ref().to_os_string())
        .collect();

    // Determine execution method based on privilege requirements
    let output = if cmd_args.first() == Some(pm.root_cmd.cmd()) {
        // Split sudo command into [sudo_cmd, actual_command...]
        pm.sudo_exec_output(
            // Actual package manager command
            &cmd_args[1],
            // Combine remaining args + packages
            &[&cmd_args[2..], &package_args].concat(),
            Some("Package operation requires root privileges"),
        )
        .await?
    } else {
        // Run without privilege escalation
        exec_output(
            // Package manager binary
            &cmd_args[0],
            // Combine subcommand + packages
            &[&cmd_args[1..], &package_args].concat(),
        )
        .await?
    };

    // Format command string for logging
    let full_cmd = cmd_args
        .iter()
        .chain(package_args.iter())
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");

    // Log outputs with consistent formatting
    log_output!(output.stdout, "Stdout", full_cmd, info);
    log_output!(output.stderr, "Stderr", full_cmd, info);

    // Validate command success status
    if output.status.success() {
        Ok(())
    } else {
        Err(eyre!(
            "Package command failed with exit code {}: {}",
            output.status.code().unwrap_or(-1),
            full_cmd
        ))
    }
}

// pub(crate) async fn find_modified_files() -> Result<> {

// }
