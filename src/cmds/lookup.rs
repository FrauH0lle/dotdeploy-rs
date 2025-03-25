use crate::store::Stores;
use crate::utils::common::bytes_to_os_str;
use color_eyre::Result;
use std::path::PathBuf;
use std::sync::Arc;

/// Lookup a file path in the dotdeploy stores and resolve to its source path
///
/// Queries both user and system stores to find the original source path for a given target file.
/// When found, resolves to the stored source path. When not found, returns the original input path.
///
/// * `file` - Target file path to look up in the stores
/// * `stores` - Combined user/system store instances to search
///
/// # Errors
/// Returns errors if:
/// * Store database access fails
/// * Path byte conversion fails
pub(crate) async fn lookup(file: PathBuf, stores: Arc<Stores>) -> Result<bool> {
    let res_file = stores
        .get_file(&file)
        .await?
        .and_then(|st_file| st_file.source_u8)
        .map(|source| PathBuf::from(bytes_to_os_str(source)))
        .unwrap_or(file);

    // Use the debug representation to return the path in order to preserve non-UTF8 chars
    println!("{:?}", res_file);
    Ok(true)
}
