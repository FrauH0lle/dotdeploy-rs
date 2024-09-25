//! This module provides default package management commands for different Linux distributions.

use std::collections::{HashMap, VecDeque};
use anyhow::Result;

/// Returns default package installation and uninstallation commands for supported distributions.
///
/// This function creates and returns two HashMaps:
/// 1. A map of distribution names to their respective package installation commands.
/// 2. A map of distribution names to their respective package uninstallation commands.
///
/// Currently supported distributions are Gentoo and Ubuntu.
///
/// # Returns
///
/// A tuple containing two HashMaps:
/// - The first HashMap contains installation commands.
/// - The second HashMap contains uninstallation commands.
///
/// Each command is represented as a VecDeque of Strings, where each String is a command argument.
///
/// # Errors
///
/// This function is infallible and always returns Ok.
pub(crate) fn default_cmds() -> Result<(
    HashMap<String, VecDeque<String>>,
    HashMap<String, VecDeque<String>>,
)> {
    // Initialize HashMap for installation commands
    let mut install_cmds: HashMap<String, VecDeque<String>> = HashMap::new();

    // Gentoo installation command
    install_cmds.insert(
        "gentoo".to_string(),
        vec![
            "sudo".to_string(),
            "emerge".to_string(),
            "--verbose".to_string(),
            "--changed-use".to_string(),
            "--deep".to_string(),
        ]
        .into(),
    );

    // Ubuntu installation command
    install_cmds.insert(
        "ubuntu".to_string(),
        vec![
            "sudo".to_string(),
            "DEBIAN_FRONTEND=noninteractive".to_string(),
            "apt-get".to_string(),
            "install".to_string(),
            "-q".to_string(),
            "-y".to_string(),
        ]
        .into(),
    );

    // Initialize HashMap for uninstallation commands
    let mut uninstall_cmds: HashMap<String, VecDeque<String>> = HashMap::new();

    // Gentoo uninstallation command
    uninstall_cmds.insert(
        "gentoo".to_string(),
        vec![
            "sudo".to_string(),
            "emerge".to_string(),
            "--deselect".to_string(),
        ]
        .into(),
    );

    // Ubuntu uninstallation command
    uninstall_cmds.insert(
        "ubuntu".to_string(),
        vec![
            "sudo".to_string(),
            "apt-get".to_string(),
            "autoremove".to_string(),
            "--purge".to_string(),
        ]
        .into(),
    );

    Ok((install_cmds, uninstall_cmds))
}
