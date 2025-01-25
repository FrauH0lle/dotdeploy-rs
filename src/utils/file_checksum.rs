//! File checksum calculation module.
//!
//! This module provides functionality to calculate SHA256 checksums of files, with built-in
//! capability to handle permission issues by elevating privileges when necessary. It's designed to
//! work in both asynchronous and synchronous contexts, utilizing Tokio for asynchronous file
//! operations and spawning blocking tasks for CPU-intensive hashing operations.

use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};
use tokio::fs;

use crate::utils::sudo;

/// Calculates the SHA256 checksum of a file, elevating privileges if necessary.
///
/// This function attempts to read the file and calculate its SHA256 checksum. If a permission error
/// is encountered, it retries the operation using sudo.
///
/// # Arguments
///
/// * `path` - The path to the file for which to calculate the checksum.
///
/// # Returns
///
/// * `Ok(String)` - The SHA256 checksum of the file as a hexadecimal string.
/// * `Err` - If an error occurs during the checksum calculation or file reading.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let checksum = calculate_sha256_checksum(Path::new("/path/to/file")).await?;
///     println!("Checksum: {}", checksum);
///     Ok(())
/// }
/// ```
pub(crate) async fn calculate_sha256_checksum<P: AsRef<Path>>(path: P) -> Result<String> {
    // Open the file asynchronously
    let checksum = match fs::read(&path).await {
        Ok(content) => {
            // If successful, perform the hashing in a blocking task. This prevents blocking the
            // async executor with CPU-intensive work
            tokio::task::spawn_blocking(move || {
                let mut hasher = Sha256::new();
                hasher.update(&content);
                // Convert the hash to a hexadecimal string
                format!("{:x}", hasher.finalize())
            })
            .await?
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            // If permission is denied, attempt to calculate checksum using sudo
            let output = sudo::sudo_exec_output("sha256sum", &[path.as_ref()], None)
                .await?
                .stdout;

            if output.is_empty() {
                bail!("sha256sum {:?} did not return any output", path.as_ref())
            } else {
                // Parse the output to extract the checksum
                String::from_utf8(output)?
                    .split_whitespace()
                    .next()
                    .ok_or_else(|| anyhow!("Failed to split whitespace"))?
                    .trim()
                    .to_string()
            }
        }
        // Propagate any other errors
        Err(e) => Err(e)
            .with_context(|| format!("Falied to calculate checksum of {:?}", &path.as_ref()))?,
    };

    Ok(checksum)
}

//
// Tests

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_calculate_sha256_checksum() -> Result<()> {
        crate::USE_SUDO.store(true, std::sync::atomic::Ordering::Relaxed);

        let temp_file = tempfile::NamedTempFile::new()?;
        let checksum = calculate_sha256_checksum(&temp_file).await?;
        assert!(!checksum.is_empty());

        // Test with elevated permissions
        sudo::sudo_exec(
            "chown",
            &["root:root", &temp_file.path().to_str().unwrap()],
            None,
        )
        .await?;
        sudo::sudo_exec("chmod", &["600", &temp_file.path().to_str().unwrap()], None).await?;

        let checksum_sudo = calculate_sha256_checksum(&temp_file).await?;
        assert_eq!(checksum, checksum_sudo);
        Ok(())
    }
}
