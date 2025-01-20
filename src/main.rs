use std::fmt::Write;

use anyhow::{Context, Result};
use log::error;

#[macro_use]
extern crate log;

mod cli;
mod config;
mod logs;

fn main() {
    match run() {
        Ok(success) if success => std::process::exit(0),
        Ok(_) => std::process::exit(1),
        Err(e) => {
            display_error(e);
            std::process::exit(1);
        }
    }
}

pub(crate) fn display_error(error: anyhow::Error) {
    let mut chain = error.chain();
    let mut error_message = format!("{}\nCaused by:\n", chain.next().unwrap());

    for e in chain {
        writeln!(error_message, "    {}", e).unwrap();
    }
    // Remove last \n
    error_message.pop();

    error!("{}", error_message);
}

fn run() -> Result<bool> {
    let cli = cli::get_cli();
    logs::init_logging(cli.verbosity)?;

    // Read config from file, if any
    let mut dotdeploy_config =
        config::DotdeployConfig::init().context("Failed to initialize Dotdeploy config")?;

    // Merge CLI args into config
    dotdeploy_config.dry_run = cli.dry_run;
    dotdeploy_config.force = cli.force;
    dotdeploy_config.noconfirm = cli.noconfirm;
    if let Some(path) = cli.config_root {
        dotdeploy_config.config_root = path;
    }
    if let Some(path) = cli.modules_root {
        dotdeploy_config.modules_root = path;
    }
    if let Some(path) = cli.hosts_root {
        dotdeploy_config.hosts_root = path;
    }
    if let Some(name) = cli.hostname {
        dotdeploy_config.hostname = name;
    }
    if let Some(name) = cli.distribution {
        dotdeploy_config.distribution = name;
    }
    dotdeploy_config.use_sudo = cli.use_sudo;
    dotdeploy_config.deploy_sys_files = cli.deploy_sys_files;
    if cli.install_pkg_cmd.is_some() {
        dotdeploy_config.install_pkg_cmd = cli.install_pkg_cmd;
    }
    if cli.remove_pkg_cmd.is_some() {
        dotdeploy_config.remove_pkg_cmd = cli.remove_pkg_cmd;
    }
    dotdeploy_config.skip_pkg_install = cli.skip_pkg_install;

    // Make config available as environment variables
    unsafe {
        std::env::set_var("DOD_DRY_RUN", &dotdeploy_config.dry_run.to_string());
        std::env::set_var("DOD_FORCE", &dotdeploy_config.force.to_string());
        std::env::set_var("DOD_YES", &dotdeploy_config.noconfirm.to_string());
        std::env::set_var("DOD_CONFIG_ROOT", &dotdeploy_config.config_root);
        std::env::set_var("DOD_MODULES_ROOT", &dotdeploy_config.modules_root);
        std::env::set_var("DOD_HOSTS_ROOT", &dotdeploy_config.hosts_root);
        std::env::set_var("DOD_HOSTNAME", &dotdeploy_config.hostname);
        std::env::set_var("DOD_DISTRIBUTION", &dotdeploy_config.distribution);
        std::env::set_var("DOD_USE_SUDO", &dotdeploy_config.use_sudo.to_string());
        std::env::set_var("DOD_DEPLOY_SYS_FILES", &dotdeploy_config.deploy_sys_files.to_string());
        std::env::set_var("DOD_SKIP_PKG_INSTALL", &dotdeploy_config.skip_pkg_install.to_string());
    }

    dbg!(&dotdeploy_config);
    println!("{:?}", std::env::var("DOD_FORCE").unwrap());
    println!("{:?}", std::env::var("DOD_MODULES_ROOT").unwrap());
    todo!("Do something ...")
}
