//! File system operations module.
//!
//! This module provides utility functions for various file system operations, including checking
//! file existence, managing symbolic links, and handling directory structures. It includes
//! functionality to elevate privileges when necessary, using sudo for operations that might require
//! higher permissions.

use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use tokio::fs;

use crate::utils::common;
use crate::utils::sudo;

/// Converts a path to a string, handling potential Unicode conversion errors.
///
/// This function is useful for operations that require string representations of paths, especially
/// when interfacing with external commands or APIs that expect strings.
///
/// # Arguments
///
/// * `path` - Any type that can be converted to a Path.
///
/// # Returns
///
/// * `Ok(String)` - The path as a valid Unicode string.
/// * `Err` - If the path contains invalid Unicode characters.
///
/// # Examples
///
/// ```
/// use std::path::Path;
/// let path_str = path_to_string(Path::new("/some/path"))?;
/// ```
pub(crate) fn path_to_string<P: AsRef<Path>>(path: P) -> Result<String> {
    let path_str = path
        .as_ref()
        .to_str()
        .ok_or_else(|| {
            anyhow!(
                "Filename {:?} contains invalid Unicode characters",
                path.as_ref()
            )
        })?
        .to_string();

    Ok(path_str)
}

/// Checks if a file exists, using sudo if necessary due to permission issues.
///
/// This function attempts to check file existence normally first, and if a permission error is
/// encountered, it retries the operation using sudo.
///
/// # Arguments
///
/// * `path` - The path to check for existence.
///
/// # Returns
///
/// * `Ok(bool)` - True if the file exists, false otherwise.
/// * `Err` - If an error occurs during the check (other than permission issues).
pub(crate) async fn check_file_exists<P: AsRef<Path>>(path: P) -> Result<bool> {
    match path.as_ref().try_exists() {
        Ok(false) => Ok(false),
        Ok(true) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            // If permission is denied, try using sudo
            Ok(sudo::sudo_exec_success("test", &["-e", &path_to_string(path)?], None).await?)
        }
        Err(e) => {
            Err(e).with_context(|| format!("Falied to check existence of {:?}", &path.as_ref()))?
        }
    }
}

/// Checks if a symbolic link exists and optionally verifies its target.
///
/// This function checks for the existence of a symbolic link and can also verify if it points to a
/// specific target. It uses sudo if necessary due to permission issues.
///
/// # Arguments
///
/// * `path` - The path to the potential symbolic link.
/// * `source` - Optional. If provided, checks if the link points to this source.
///
/// # Returns
///
/// * `Ok(bool)` - True if the link exists (and points to the specified source, if provided).
/// * `Err` - If an error occurs during the check.
pub(crate) async fn check_link_exists<P: AsRef<Path>>(path: P, source: Option<P>) -> Result<bool> {
    match fs::symlink_metadata(path.as_ref()).await {
        Ok(meta) => {
            if let Some(s) = source {
                if meta.is_symlink() {
                    let orig = fs::read_link(path).await?;
                    Ok(&orig == s.as_ref())
                } else {
                    Ok(false)
                }
            } else {
                Ok(meta.is_symlink())
            }
        }

        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            // If permission is denied, use sudo for the check
            if let Some(s) = source {
                if sudo::sudo_exec_success("test", &["-L", &path_to_string(&path)?], None).await? {
                    let orig = String::from_utf8(
                        sudo::sudo_exec_output("readlink", &[path_to_string(&path)?], None)
                            .await?
                            .stdout,
                    )?
                    .trim()
                    .to_string();
                    Ok(&orig.as_ref() == s.as_ref())
                } else {
                    Ok(false)
                }
            } else {
                Ok(sudo::sudo_exec_success("test", &["-L", &path_to_string(&path)?], None).await?)
            }
        }
        Err(e) => {
            Err(e).with_context(|| format!("Falied to check existence of {:?}", &path.as_ref()))?
        }
    }
}

/// Ensures that a directory exists, creating it if necessary, using sudo if needed.
///
/// This function attempts to create a directory and all its parent directories. If a permission
/// error is encountered, it retries the operation using sudo.
///
/// # Arguments
///
/// * `path` - The path of the directory to ensure exists.
///
/// # Returns
///
/// * `Ok(())` - If the directory exists or was successfully created.
/// * `Err` - If an error occurs during the operation.
pub(crate) async fn ensure_dir_exists<P: AsRef<Path>>(path: P) -> Result<()> {
    match fs::create_dir_all(&path).await {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            // If permission is denied, use sudo to create the directory
            Ok(sudo::sudo_exec("mkdir", &["-p", &path_to_string(&path)?], None).await?)
        }
        Err(e) => Err(e).with_context(|| format!("Falied to delete {:?}", &path.as_ref()))?,
    }
}

/// Deletes a file, using sudo if necessary due to permission issues.
///
/// This function attempts to delete a file normally first, and if a permission error is
/// encountered, it retries the operation using sudo.
///
/// # Arguments
///
/// * `path` - The path of the file to delete.
///
/// # Returns
///
/// * `Ok(())` - If the file was successfully deleted or didn't exist.
/// * `Err` - If an error occurs during the deletion.
pub(crate) async fn delete_file<P: AsRef<Path>>(path: P) -> Result<()> {
    match fs::remove_file(&path).await {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            Ok(sudo::sudo_exec("rm", &["-f", &path_to_string(&path)?], None).await?)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            warn!("{}", e);
            Ok(())
        }
        Err(e) => Err(e).with_context(|| format!("Falied to delete {:?}", &path.as_ref()))?,
    }
}

/// Recursively deletes empty parent directories, optionally prompting for confirmation.
///
/// This function walks up the directory tree from the given path, deleting empty directories. It
/// can either ask for confirmation before each deletion or proceed without asking.
///
/// # Arguments
///
/// * `path` - The starting path from which to begin deleting empty parent directories.
/// * `no_ask` - If true, deletes without asking for confirmation. If false, prompts before each
///   deletion.
///
/// # Returns
///
/// * `Ok(())` - If all operations were successful.
/// * `Err` - If an error occurs during the process.
pub(crate) async fn delete_parents<P: AsRef<Path>>(path: P, no_ask: bool) -> Result<()> {
    let mut path = path
        .as_ref()
        .parent()
        .with_context(|| format!("Failed to get parent of {:?}", path.as_ref()))?;

    while path.is_dir()
        && match path.read_dir() {
            Ok(_) => path
                .read_dir()
                .map(|mut i| i.next().is_none())
                .unwrap_or(false),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                // If permission is denied, use sudo to check if the directory is empty
                let path_str = path_to_string(path)?;
                let output = sudo::sudo_exec_output(
                    "find",
                    &[path_str.as_str(), "-maxdepth", "0", "-empty"],
                    None,
                )
                .await?;
                if output.status.success() {
                    !output.stdout.is_empty()
                } else {
                    bail!(
                        "Failed to check if directory {} is empty: {}",
                        path_str,
                        String::from_utf8(output.stderr)?
                    )
                }
            }
            Err(e) => Err(e).with_context(|| format!("Failed to read directory {:?}", path))?,
        }
    {
        if no_ask
            || common::ask_boolean(&format!(
                "Directory at {:?} is now empty. Delete [y/N]? ",
                path
            ))
        {
            match fs::remove_dir(path).await {
                Ok(_) => (),
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    // If permission is denied, use sudo to remove the directory
                    sudo::sudo_exec("rmdir", &[&path_to_string(path)?], None).await?
                }
                Err(e) => {
                    Err(e).with_context(|| format!("Failed to remove directory {:?}", path))?
                }
            }
        }
        path = path
            .parent()
            .with_context(|| format!("Failed to get parent of {:?}", path))?;
    }
    Ok(())
}

//
// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_path_to_string() -> Result<()> {
        assert_eq!(
            path_to_string(PathBuf::from("/foo/bar.txt"))?,
            "/foo/bar.txt".to_string()
        );
        assert_eq!(
            path_to_string(Path::new("/foo/bar.txt"))?,
            "/foo/bar.txt".to_string()
        );
        // Test for invalid unicode character, adapted from
        // https://github.com/rust-lang/cargo/pull/9226/files#diff-9977238c61100eb9f319febd88e2434b304ac401f0da3b50d00d7c91de319e2fR2957-R2966
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        assert!(path_to_string(PathBuf::from(OsString::from_vec(vec![255]))).is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_check_file_exists() -> Result<()> {
        crate::USE_SUDO.store(true, std::sync::atomic::Ordering::Relaxed);

        let temp_file = tempfile::NamedTempFile::new()?;
        assert!(check_file_exists(temp_file).await?);
        assert!(!check_file_exists("/tmp/doesnotexist.txt").await?);

        // Test with elevated permissions
        let temp_dir = tempfile::tempdir()?;
        let temp_file = temp_dir.path().join("test.txt");
        fs::File::create(&temp_file).await?;
        sudo::sudo_exec(
            "chown",
            &["root:root", &temp_dir.path().to_str().unwrap()],
            None,
        )
        .await?;
        sudo::sudo_exec("chmod", &["600", &temp_dir.path().to_str().unwrap()], None).await?;
        assert!(check_file_exists(temp_file).await?);
        assert!(!check_file_exists(temp_dir.path().join("doesnotexist.txt")).await?);
        Ok(())
    }

    #[tokio::test]
    async fn test_check_link_exists() -> Result<()> {
        crate::USE_SUDO.store(true, std::sync::atomic::Ordering::Relaxed);

        let temp_file = tempfile::NamedTempFile::new()?;
        let temp_file_pathbuf = PathBuf::from(&temp_file.path());
        let temp_dir = tempfile::tempdir()?;
        let temp_link = temp_dir.path().join("foo.txt");
        fs::symlink(&temp_file, &temp_link).await?;
        assert!(check_link_exists(&temp_link, Some(&temp_file_pathbuf)).await?);
        assert!(check_link_exists(&temp_link, None).await?);

        // Test with elevated permissions
        let temp_file = tempfile::NamedTempFile::new()?;
        let temp_file_pathbuf = PathBuf::from(&temp_file.path());
        let temp_dir = tempfile::tempdir()?;
        let temp_link = temp_dir.path().join("foo.txt");
        fs::symlink(&temp_file, &temp_link).await?;

        sudo::sudo_exec(
            "chown",
            &["root:root", &temp_dir.path().to_str().unwrap()],
            None,
        )
        .await?;
        sudo::sudo_exec("chmod", &["600", &temp_dir.path().to_str().unwrap()], None).await?;

        assert!(check_link_exists(&temp_link, Some(&temp_file_pathbuf)).await?);
        assert!(check_link_exists(&temp_link, None).await?);
        Ok(())
    }

    #[tokio::test]
    async fn test_ensure_dir() -> Result<()> {
        crate::USE_SUDO.store(true, std::sync::atomic::Ordering::Relaxed);

        let temp_dir = tempfile::tempdir()?;
        let target = temp_dir.path().join("a").join("b").join("c");
        ensure_dir_exists(&target).await?;
        assert!(check_file_exists(&target).await?);
        assert!(check_file_exists(&target).await?);

        // Test with elevated permissions
        let temp_dir = tempfile::tempdir()?;
        let target = temp_dir.path().join("a").join("b").join("c");

        sudo::sudo_exec(
            "chown",
            &["root:root", &temp_dir.path().to_str().unwrap()],
            None,
        )
        .await?;
        sudo::sudo_exec("chmod", &["600", &temp_dir.path().to_str().unwrap()], None).await?;

        ensure_dir_exists(&target).await?;
        assert!(check_file_exists(&target).await?);
        assert!(check_file_exists(&target).await?);
        Ok(())
    }

    #[tokio::test]
    async fn test_delete_file() -> Result<()> {
        crate::USE_SUDO.store(true, std::sync::atomic::Ordering::Relaxed);

        let temp_file = tempfile::NamedTempFile::new()?;

        assert!(check_file_exists(&temp_file).await?);
        assert!(delete_file(&temp_file).await.is_ok());
        assert!(!check_file_exists(&temp_file).await?);
        // Return Ok(()) if file is not found
        assert!(delete_file(&temp_file).await.is_ok());

        // Test with elevated permissions
        let temp_file = tempfile::NamedTempFile::new()?;

        sudo::sudo_exec(
            "chown",
            &["root:root", &temp_file.path().to_str().unwrap()],
            None,
        )
        .await?;
        sudo::sudo_exec("chmod", &["600", &temp_file.path().to_str().unwrap()], None).await?;

        assert!(check_file_exists(&temp_file).await?);
        assert!(delete_file(&temp_file).await.is_ok());
        assert!(!check_file_exists(&temp_file).await?);
        // Return Ok(()) if file is not found
        assert!(delete_file(&temp_file).await.is_ok());
        Ok(())
    }

    #[tokio::test]
    async fn test_delete_parents() -> Result<()> {
        crate::USE_SUDO.store(true, std::sync::atomic::Ordering::Relaxed);

        let temp_dir = tempfile::tempdir()?;
        let temp_path1 = temp_dir.path().join("test1");
        fs::create_dir(&temp_path1).await?;
        assert!(&temp_path1.exists());

        let temp_path2 = temp_dir.path().join("test 2");
        fs::create_dir(&temp_path2).await?;
        assert!(&temp_path2.exists());

        // Create a file
        fs::write(&temp_path1.join("text.txt"), "hi").await?;
        // Try to delete non-empty dir
        delete_parents(&temp_path1.join("text.txt"), true).await?;
        assert!(&temp_path1.exists());
        // Remove file and try again
        fs::remove_file(&temp_path1.join("text.txt")).await?;
        delete_parents(&temp_path1.join("text.txt"), true).await?;
        assert!(!&temp_path1.exists());
        delete_parents(&temp_path2.join("text.txt"), true).await?;
        assert!(!&temp_path2.exists());
        // Verify that the grandparent got deleted as well
        assert!(!&temp_dir.path().exists());

        // Test with elevated permissions
        let temp_dir = tempfile::tempdir()?;
        let temp_path1 = temp_dir.path().join("test1");
        fs::create_dir(&temp_path1).await?;
        assert!(&temp_path1.exists());

        let temp_path2 = temp_dir.path().join("test 2");
        fs::create_dir(&temp_path2).await?;
        assert!(&temp_path2.exists());

        // Create a file
        fs::write(&temp_path1.join("text.txt"), "hi").await?;
        // Change owner and permissions
        sudo::sudo_exec("chown", &["root:root", &temp_path1.to_str().unwrap()], None).await?;
        sudo::sudo_exec("chown", &["root:root", &temp_path2.to_str().unwrap()], None).await?;
        sudo::sudo_exec("chmod", &["600", &temp_path1.to_str().unwrap()], None).await?;
        sudo::sudo_exec("chmod", &["600", &temp_path2.to_str().unwrap()], None).await?;

        // Try to delete non-empty dir
        delete_parents(&temp_path1.join("text.txt"), true).await?;
        assert!(&temp_path1.exists());
        // Remove file and try again
        sudo::sudo_exec(
            "rm",
            &["-f", &temp_path1.join("text.txt").to_str().unwrap()],
            None,
        )
        .await?;
        delete_parents(&temp_path1.join("text.txt"), true).await?;
        assert!(!&temp_path1.exists());
        delete_parents(&temp_path2.join("text.txt"), true).await?;
        assert!(!&temp_path2.exists());
        // Verify that the grandparent got deleted as well
        assert!(!&temp_dir.path().exists());
        Ok(())
    }
}
