use std::collections::{HashMap, VecDeque};

use anyhow::Result;

pub(crate) fn default_cmds() -> Result<(
    HashMap<String, VecDeque<String>>,
    HashMap<String, VecDeque<String>>,
)> {
    let mut install_cmds: HashMap<String, VecDeque<String>> = HashMap::new();
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

    let mut uninstall_cmds: HashMap<String, VecDeque<String>> = HashMap::new();
    uninstall_cmds.insert(
        "gentoo".to_string(),
        vec![
            "sudo".to_string(),
            "emerge".to_string(),
            "--deselect".to_string(),
        ]
        .into(),
    );
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
