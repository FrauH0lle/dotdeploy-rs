use crate::store::sqlite::SQLiteStore;
use crate::store::Store;
use crate::utils::common::bytes_to_os_str;
use color_eyre::Result;
use std::path::PathBuf;
use std::sync::Arc;

/// Lookup a file path in the dotdeploy store and resolve to its source path
///
/// Queries user store to find the original source path for a given target file. When found,
/// resolves to the stored source path. When not found, returns the original input path.
///
/// * `file` - Target file path to look up in the stores
/// * `store` - User store instances to search
///
/// # Errors
/// Returns errors if:
/// * Store database access fails
/// * Path byte conversion fails
pub(crate) async fn lookup(file: PathBuf, store: Arc<SQLiteStore>) -> Result<bool> {
    let res_file = store
        .get_file(&file)
        .await?
        .and_then(|st_file| st_file.source_u8)
        .map(|source| PathBuf::from(bytes_to_os_str(source)))
        .unwrap_or(file);

    // Use the debug representation to return the path in order to preserve non-UTF8 chars
    println!("{:?}", res_file);
    Ok(true)
}
