use anyhow::{bail, Result};
use tempfile::tempdir;

use crate::store::db::Store;
use crate::store::files::StoreFile;
use crate::store::init::init_user_store;
use crate::store::modules::StoreModule;

pub(crate) async fn store_setup_helper(op_tye: &str) -> Result<Store> {
    let temp_dir = tempdir()?;

    // Initialize the user store, which sets up the database and tables
    let pool = init_user_store(Some(temp_dir.into_path()))
        .await
        .map_err(|e| e.into_anyhow())?;

    // Insert a module
    let test_module = StoreModule {
        name: "test".to_string(),
        location: "/testpath".to_string(),
        user: Some("user".to_string()),
        reason: "manual".to_string(),
        depends: None,
        date: chrono::offset::Local::now(),
    };

    pool.add_module(test_module)
        .await
        .map_err(|e| e.into_anyhow())?;

    for i in 0..5 {
        let local_time = chrono::offset::Local::now();
        let test_file = StoreFile {
            module: "test".to_string(),
            source: match op_tye {
                "link" => Some(format!("/dotfiles/foo{}.txt", i)),
                "copy" => Some(format!("/dotfiles/foo{}.txt", i)),
                "create" => None,
                _ => bail!("Invalid 'which' parameter. Must be either 'link', 'copy' or 'create'."),
            },
            source_checksum: match op_tye {
                "link" => Some(format!("source_checksum{}", i)),
                "copy" => Some(format!("source_checksum{}", i)),
                "create" => None,
                _ => bail!("Invalid 'which' parameter. Must be either 'link', 'copy' or 'create'."),
            },
            destination: format!("/home/foo{}.txt", i),
            destination_checksum: Some(format!("dest_checksum{}", i)),
            operation: match op_tye {
                "link" => "link".to_string(),
                "copy" => "copy".to_string(),
                "create" => "create".to_string(),
                _ => bail!("Invalid 'which' parameter. Must be either 'link', 'copy' or 'create'."),
            },
            user: Some("user".to_string()),
            date: local_time,
        };

        pool.add_file(test_file)
            .await
            .map_err(|e| e.into_anyhow())?;
    }

    Ok(pool)
}
