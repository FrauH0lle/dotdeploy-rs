use std::sync::mpsc::{self, Sender};
use crate::utils::sudo::{GetRootCmd, PrivilegeManager, PrivilegeManagerBuilder};
use color_eyre::Result;
use std::sync::{Arc, RwLock};

pub(crate) fn pm_setup() -> Result<(Sender<()>, Arc<PrivilegeManager>)> {
    let (tx, rx) = mpsc::channel();

    let pm = Arc::new(
        PrivilegeManagerBuilder::new()
            .with_use_sudo(true)
            .with_root_cmd(GetRootCmd::use_sudo())
            .with_terminal_lock(Arc::new(RwLock::new(())))
            .with_channel_rx(Some(rx))
            .build()?,
    );

    Ok((tx, pm))
}
