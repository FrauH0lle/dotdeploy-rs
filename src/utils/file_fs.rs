//! File system operations module.
//!
//! This module provides utility functions for various file system operations, including checking
//! file existence, managing symbolic links, and handling directory structures. It includes
//! functionality to elevate privileges when necessary, using sudo for operations that might require
//! higher permissions.

use crate::utils::FileUtils;
use crate::utils::common;
use color_eyre::Result;
use color_eyre::eyre::{WrapErr, eyre};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{instrument, warn};

/// Converts a path to a string, handling potential Unicode conversion errors.
///
/// This function is useful for operations that require string representations of paths, especially
/// when interfacing with external commands or APIs that expect strings.
///
/// # Arguments
///
/// * `path` - Any type that can be converted to a Path.
///
/// # Errors
/// Returns an error if the path contains invalid Unicode characters.
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
            eyre!(
                "Filename {:?} contains invalid Unicode characters",
                path.as_ref()
            )
        })?
        .to_string();

    Ok(path_str)
}

/// Expands a path string, resolving environment variables and tilde expressions.
///
/// This function takes a string slice and expands any environment variables (e.g., $HOME) and tilde
/// expressions (~) within it.
///
/// # Arguments
///
/// * `path` - A str
/// * `env` - Optional HashMap containing environment variable pairs to prepend to the default
///           environment
///
/// # Errors
///
/// Returns an error if path contains invalid Unicode or environment variables cannot be expanded.
pub(crate) fn expand_path_string(
    path: &str,
    env: Option<&HashMap<String, String>>,
) -> Result<String> {
    let home_dir = || -> Option<String> {
        let hd = dirs::home_dir()?;
        path_to_string(hd).ok()
    };

    // Create variable lookup closure that checks custom env first, then system env
    let context = |var: &str| -> Result<Option<String>, std::env::VarError> {
        // Check custom environment variables first
        if let Some(custom_env) = &env {
            if let Some(value) = custom_env.get(var) {
                return Ok(Some(value.clone()));
            }
        }
        // Fall back to system environment variables
        std::env::var(var).map(Some)
    };

    // Expand the path using shellexpand with custom context
    let expanded = shellexpand::full_with_context(path, home_dir, context)
        .map_err(|e| eyre!("Failed to expand path: {:?}", e))?;

    Ok(expanded.to_string())
}

/// Expands a path string, resolving environment variables and tilde expressions.
///
/// This function takes a path and expands any environment variables (e.g., $HOME) and tilde
/// expressions (~) within it.
///
/// # Arguments
///
/// * `path` - Any type that can be converted to a Path
/// * `env` - Optional HashMap containing environment variable pairs to prepend to the default
///           environment
///
/// # Errors
///
/// Returns an error if path contains invalid Unicode or environment variables cannot be expanded.
pub(crate) fn expand_path<P: AsRef<Path>>(
    path: P,
    env: Option<&HashMap<String, String>>,
) -> Result<PathBuf> {
    // Convert path to string, handling Unicode conversion
    let path_str = path_to_string(path)?;
    // Expand path string
    let expanded = expand_path_string(&path_str, env)?;

    Ok(PathBuf::from(expanded))
}

impl FileUtils {
    /// Checks if a file exists, using sudo if necessary due to permission issues.
    ///
    /// This function attempts to check file existence normally first, and if a permission error is
    /// encountered, it retries the operation using sudo.
    ///
    /// # Arguments
    ///
    /// * `path` - The path to check for existence.
    ///
    /// # Errors
    ///
    /// Returns an error if an error occurs during the check (other than permission issues).
    pub(crate) async fn check_file_exists<P: AsRef<Path>>(&self, path: P) -> Result<bool> {
        match path.as_ref().try_exists() {
            Ok(false) => Ok(false),
            Ok(true) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                // If permission is denied, try using sudo
                Ok(self
                    .privilege_manager
                    .sudo_exec_success("test", ["-e", &path_to_string(path)?], None)
                    .await?)
            }
            Err(e) => Err(e)
                .wrap_err_with(|| format!("Failed to check existence of {:?}", &path.as_ref()))?,
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
    /// # Errors
    ///
    /// Returns an error if an error occurs during the check.
    pub(crate) async fn check_link_exists<P: AsRef<Path>>(
        &self,
        path: P,
        source: Option<P>,
    ) -> Result<bool> {
        match fs::symlink_metadata(path.as_ref()).await {
            Ok(meta) => match source {
                Some(s) => {
                    if meta.is_symlink() {
                        let orig = fs::read_link(path).await?;
                        Ok(orig == *s.as_ref())
                    } else {
                        Ok(false)
                    }
                }
                _ => Ok(meta.is_symlink()),
            },

            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                // If permission is denied, use sudo for the check
                match source {
                    Some(s) => {
                        if self
                            .privilege_manager
                            .sudo_exec_success("test", ["-L", &path_to_string(&path)?], None)
                            .await?
                        {
                            let orig = String::from_utf8(
                                self.privilege_manager
                                    .sudo_exec_output(
                                        "readlink",
                                        [path_to_string(&path)?.as_str()],
                                        None,
                                    )
                                    .await?
                                    .stdout,
                            )?
                            .trim()
                            .to_string();
                            Ok(orig.as_ref() == *s.as_ref())
                        } else {
                            Ok(false)
                        }
                    }
                    _ => Ok(self
                        .privilege_manager
                        .sudo_exec_success("test", ["-L", &path_to_string(&path)?], None)
                        .await?),
                }
            }
            Err(e) => Err(e)
                .wrap_err_with(|| format!("Failed to check existence of {:?}", &path.as_ref()))?,
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
    /// # Errors
    ///
    /// Returns an error if an error occurs during the operation.
    pub(crate) async fn ensure_dir_exists<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        match fs::create_dir_all(&path).await {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                // If permission is denied, use sudo to create the directory
                Ok(self
                    .privilege_manager
                    .sudo_exec("mkdir", ["-p", &path_to_string(&path)?], None)
                    .await?)
            }
            Err(e) => Err(e).wrap_err_with(|| format!("Failed to create {:?}", &path.as_ref()))?,
        }
    }

    /// Copies a file, using sudo if necessary due to permission issues.
    ///
    /// This function attempts to copy a file normally first, and if a permission error is
    /// encountered, it retries the operation using sudo.
    ///
    /// # Arguments
    ///
    /// * `from` - Source path to copy from
    /// * `to` - Destination path to copy to
    ///
    /// # Errors
    /// Returns an error if:
    /// - Source file cannot be read
    /// - Destination directory cannot be created
    /// - Copy operation fails even with elevated privileges
    pub(crate) async fn copy_file<P: AsRef<Path>>(&self, from: P, to: P) -> Result<()> {
        self.ensure_dir_exists(
            &to.as_ref()
                .parent()
                .ok_or_else(|| eyre!("Could not get parent of {}", &to.as_ref().display()))?,
        )
        .await?;

        // Remove existing file or symlink if it exists
        if self.check_file_exists(&to).await? {
            self.delete_file(&to).await?
        }

        match fs::copy(&from, &to).await {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Ok(self
                .privilege_manager
                .sudo_exec(
                    "cp",
                    [
                        path_to_string(&from)?.as_str(),
                        path_to_string(&to)?.as_str(),
                    ],
                    Some(&format!(
                        "Copy {} -> {}",
                        &from.as_ref().display(),
                        &to.as_ref().display()
                    )),
                )
                .await?),
            Err(e) => Err(e).wrap_err_with(|| {
                format!(
                    "Failed to copy {} -> {}",
                    &from.as_ref().display(),
                    &to.as_ref().display()
                )
            })?,
        }
    }

    /// Links a file, using sudo if necessary due to permission issues.
    ///
    /// This function attempts to link a file normally first, and if a permission error is
    /// encountered, it retries the operation using sudo.
    ///
    /// # Arguments
    ///
    /// * `from` - Source path to copy from
    /// * `to` - Destination path to copy to
    ///
    /// # Errors
    /// Returns an error if:
    /// - Source file cannot be read
    /// - Destination directory cannot be created
    /// - Copy operation fails even with elevated privileges
    pub(crate) async fn link_file<P: AsRef<Path>>(&self, from: P, to: P) -> Result<()> {
        self.ensure_dir_exists(
            &to.as_ref()
                .parent()
                .ok_or_else(|| eyre!("Could not get parent of {}", &to.as_ref().display()))?,
        )
        .await?;

        // Remove existing file or symlink if it exists
        if self.check_file_exists(&to).await? {
            self.delete_file(&to).await?
        }

        match fs::symlink(&from, &to).await {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Ok(self
                .privilege_manager
                .sudo_exec(
                    "ln",
                    ["-sf", &path_to_string(&from)?, &path_to_string(&to)?],
                    Some(&format!(
                        "Link {} -> {}",
                        &from.as_ref().display(),
                        &to.as_ref().display()
                    )),
                )
                .await?),
            Err(e) => Err(e).wrap_err_with(|| {
                format!(
                    "Failed to link {} -> {}",
                    &from.as_ref().display(),
                    &to.as_ref().display()
                )
            })?,
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
    /// # Errors
    ///
    /// Returns an error if an error occurs during the deletion.
    #[instrument(skip(path))]
    pub(crate) async fn delete_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        match fs::remove_file(&path).await {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Ok(self
                .privilege_manager
                .sudo_exec("rm", ["-f", &path_to_string(&path)?], None)
                .await?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                warn!("{}: {}", path.as_ref().display(), e);
                Ok(())
            }
            Err(e) => Err(e).wrap_err_with(|| format!("Failed to delete {:?}", &path.as_ref()))?,
        }
    }

    /// Recursively deletes empty parent directories, optionally prompting for confirmation.
    ///
    /// This function walks up the directory tree from the given path, deleting empty directories.
    /// It can either ask for confirmation before each deletion or proceed without asking.
    ///
    /// # Arguments
    ///
    /// * `path` - The starting path from which to begin deleting empty parent directories.
    /// * `no_ask` - If true, deletes without asking for confirmation. If false, prompts before each
    ///   deletion.
    ///
    /// # Errors
    ///
    /// Returns an error if an error occurs during the process.
    pub(crate) async fn delete_parents<P: AsRef<Path>>(&self, path: P, no_ask: bool) -> Result<()> {
        let mut path = path
            .as_ref()
            .parent()
            .ok_or_else(|| eyre!("Failed to get parent of {:?}", path.as_ref()))?;

        while path.is_dir()
            && match path.read_dir() {
                Ok(_) => path
                    .read_dir()
                    .map(|mut i| i.next().is_none())
                    .unwrap_or(false),
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    // If permission is denied, use sudo to check if the directory is empty
                    let path_str = path_to_string(path)?;
                    let output = self
                        .privilege_manager
                        .sudo_exec_output(
                            "find",
                            [path_str.as_str(), "-maxdepth", "0", "-empty"],
                            None,
                        )
                        .await?;
                    if output.status.success() {
                        !output.stdout.is_empty()
                    } else {
                        return Err(eyre!(
                            "Failed to check if directory {} is empty: {}",
                            path_str,
                            String::from_utf8(output.stderr)?
                        ));
                    }
                }
                Err(e) => {
                    Err(e).wrap_err_with(|| format!("Failed to read directory {:?}", path))?
                }
            }
        {
            if no_ask
                || common::ask_boolean(&format!(
                    "{}\n{}",
                    format!("Directory {} is now empty. Delete [y/N]?", path.display()),
                    "(You can skip this prompt with the CLI argument '-y true' or '--noconfirm=true')",
                ))
            {
                match fs::remove_dir(path).await {
                    Ok(_) => (),
                    Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                        // If permission is denied, use sudo to remove the directory
                        self.privilege_manager
                            .sudo_exec("rmdir", [path_to_string(path)?.as_str()], None)
                            .await?
                    }
                    Err(e) => {
                        Err(e).wrap_err_with(|| format!("Failed to remove directory {:?}", path))?
                    }
                }
            }
            path = path
                .parent()
                .ok_or_else(|| eyre!("Failed to get parent of {:?}", path))?;
        }
        Ok(())
    }
}

/// Reads all files in a directory recursively.
///
/// This function traverses the given directory and all its subdirectories, collecting the paths of
/// all files encountered.
///
/// # Arguments
///
/// * `path` - The path of the directory to read.
///
/// # Errors
///
/// Returns an error if an error occurs during the operation.
pub(crate) fn read_directory<P: AsRef<Path>>(path: P) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            // If it's a directory, recursively read its contents
            files.extend(read_directory(&path)?);
        } else {
            // If it's a file, add it to the list
            files.push(path)
        }
    }
    Ok(files)
}

// -------------------------------------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests;
    use color_eyre::eyre::OptionExt;
    use std::sync::Arc;

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

    #[test]
    fn test_expand_path() -> Result<()> {
        // Test with tilde expansion
        let home = dirs::home_dir().ok_or_eyre("Failed to get HOME dir")?;
        assert_eq!(
            expand_path("~/test.txt", None)?,
            PathBuf::from(format!("{}/test.txt", path_to_string(home)?))
        );

        // Test with environment variable
        let mut env = HashMap::new();
        env.insert("TEST_DIR".to_string(), "/tmp/test".to_string());
        assert_eq!(
            expand_path("$TEST_DIR/file.txt", Some(&env))?,
            PathBuf::from("/tmp/test/file.txt")
        );

        // Test with both tilde and env var
        let home = dirs::home_dir().ok_or_eyre("Failed to get HOME dir")?;
        let mut env = HashMap::new();
        env.insert("TEST_DIR".to_string(), "/tmp/test".to_string());
        assert_eq!(
            expand_path("~/dir/$TEST_DIR/file.txt", Some(&env))?,
            PathBuf::from(format!("{}/dir/tmp/test/file.txt", path_to_string(home)?))
        );

        // Test with absolute path (no expansion needed)
        assert_eq!(
            expand_path("/absolute/path.txt", None)?,
            PathBuf::from("/absolute/path.txt")
        );

        // Test with invalid UTF-8
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        assert!(expand_path(PathBuf::from(OsString::from_vec(vec![255])), None).is_err());

        Ok(())
    }

    #[tokio::test]
    async fn test_check_file_exists() -> Result<()> {
        let (_tx, pm) = tests::pm_setup()?;
        let fs_utils = FileUtils::new(Arc::clone(&pm));

        let temp_file = tempfile::NamedTempFile::new()?;
        assert!(fs_utils.check_file_exists(temp_file).await?);
        assert!(!fs_utils.check_file_exists("/tmp/doesnotexist.txt").await?);

        // Test with elevated permissions
        let temp_dir = tempfile::tempdir()?;
        let temp_file = temp_dir.path().join("test.txt");
        fs::File::create(&temp_file).await?;
        pm.sudo_exec(
            "chown",
            ["root:root", temp_dir.path().to_str().unwrap()],
            None,
        )
        .await?;
        pm.sudo_exec("chmod", ["600", temp_dir.path().to_str().unwrap()], None)
            .await?;
        assert!(fs_utils.check_file_exists(temp_file).await?);
        assert!(
            !fs_utils
                .check_file_exists(temp_dir.path().join("doesnotexist.txt"))
                .await?
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_check_link_exists() -> Result<()> {
        let (_tx, pm) = tests::pm_setup()?;
        let fs_utils = FileUtils::new(Arc::clone(&pm));

        let temp_file = tempfile::NamedTempFile::new()?;
        let temp_file_pathbuf = PathBuf::from(&temp_file.path());
        let temp_dir = tempfile::tempdir()?;
        let temp_link = temp_dir.path().join("foo.txt");
        fs::symlink(&temp_file, &temp_link).await?;
        assert!(
            fs_utils
                .check_link_exists(&temp_link, Some(&temp_file_pathbuf))
                .await?
        );
        assert!(fs_utils.check_link_exists(&temp_link, None).await?);

        // Test with elevated permissions
        let temp_file = tempfile::NamedTempFile::new()?;
        let temp_file_pathbuf = PathBuf::from(&temp_file.path());
        let temp_dir = tempfile::tempdir()?;
        let temp_link = temp_dir.path().join("foo.txt");
        fs::symlink(&temp_file, &temp_link).await?;

        pm.sudo_exec(
            "chown",
            ["root:root", temp_dir.path().to_str().unwrap()],
            None,
        )
        .await?;
        pm.sudo_exec("chmod", ["600", temp_dir.path().to_str().unwrap()], None)
            .await?;

        assert!(
            fs_utils
                .check_link_exists(&temp_link, Some(&temp_file_pathbuf))
                .await?
        );
        assert!(fs_utils.check_link_exists(&temp_link, None).await?);
        Ok(())
    }

    #[test]
    fn test_read_directory() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let files = read_directory(temp_dir.path())?;
        assert!(files.is_empty());

        let temp_dir = tempfile::tempdir()?;
        std::fs::File::create(temp_dir.path().join("file1.txt"))?;
        std::fs::File::create(temp_dir.path().join("file2.txt"))?;

        let files = read_directory(temp_dir.path())?;
        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|f| f.file_name().unwrap() == "file1.txt"));
        assert!(files.iter().any(|f| f.file_name().unwrap() == "file2.txt"));

        let temp_dir = tempfile::tempdir()?;
        std::fs::create_dir_all(temp_dir.path().join("test1").join("test2"))?;
        std::fs::File::create(temp_dir.path().join("file1.txt"))?;
        std::fs::File::create(temp_dir.path().join("file2.txt"))?;
        std::fs::File::create(temp_dir.path().join("test1").join("file3.txt"))?;
        std::fs::File::create(
            temp_dir
                .path()
                .join("test1")
                .join("test2")
                .join("file4.txt"),
        )?;

        let files = read_directory(temp_dir.path())?;
        assert_eq!(files.len(), 4, "Should find 4 files");
        assert!(files.iter().any(|f| f.file_name().unwrap() == "file1.txt"));
        assert!(files.iter().any(|f| f.file_name().unwrap() == "file2.txt"));
        assert!(files.iter().any(|f| f.file_name().unwrap() == "file3.txt"));
        assert!(files.iter().any(|f| f.file_name().unwrap() == "file4.txt"));

        Ok(())
    }

    #[tokio::test]
    async fn test_ensure_dir_exists() -> Result<()> {
        let (_tx, pm) = tests::pm_setup()?;
        let fs_utils = FileUtils::new(Arc::clone(&pm));

        let temp_dir = tempfile::tempdir()?;
        let target = temp_dir.path().join("a").join("b").join("c");
        fs_utils.ensure_dir_exists(&target).await?;
        assert!(fs_utils.check_file_exists(&target).await?);
        assert!(fs_utils.check_file_exists(&target).await?);

        // Test with elevated permissions
        let temp_dir = tempfile::tempdir()?;
        let target = temp_dir.path().join("a").join("b").join("c");

        pm.sudo_exec(
            "chown",
            ["root:root", temp_dir.path().to_str().unwrap()],
            None,
        )
        .await?;
        pm.sudo_exec("chmod", ["600", temp_dir.path().to_str().unwrap()], None)
            .await?;

        fs_utils.ensure_dir_exists(&target).await?;
        assert!(fs_utils.check_file_exists(&target).await?);
        assert!(fs_utils.check_file_exists(&target).await?);
        Ok(())
    }

    #[tokio::test]
    async fn test_delete_file() -> Result<()> {
        let (_tx, pm) = tests::pm_setup()?;
        let fs_utils = FileUtils::new(Arc::clone(&pm));

        let temp_file = tempfile::NamedTempFile::new()?;

        assert!(fs_utils.check_file_exists(&temp_file).await?);
        assert!(fs_utils.delete_file(&temp_file).await.is_ok());
        assert!(!fs_utils.check_file_exists(&temp_file).await?);
        // Return Ok(()) if file is not found
        assert!(fs_utils.delete_file(&temp_file).await.is_ok());

        // Test with elevated permissions
        let temp_file = tempfile::NamedTempFile::new()?;

        pm.sudo_exec(
            "chown",
            ["root:root", temp_file.path().to_str().unwrap()],
            None,
        )
        .await?;
        pm.sudo_exec("chmod", ["600", temp_file.path().to_str().unwrap()], None)
            .await?;

        assert!(fs_utils.check_file_exists(&temp_file).await?);
        assert!(fs_utils.delete_file(&temp_file).await.is_ok());
        assert!(!fs_utils.check_file_exists(&temp_file).await?);
        // Return Ok(()) if file is not found
        assert!(fs_utils.delete_file(&temp_file).await.is_ok());
        Ok(())
    }

    #[tokio::test]
    async fn test_delete_parents() -> Result<()> {
        let (_tx, pm) = tests::pm_setup()?;
        let fs_utils = FileUtils::new(Arc::clone(&pm));

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
        fs_utils
            .delete_parents(&temp_path1.join("text.txt"), true)
            .await?;
        assert!(&temp_path1.exists());
        // Remove file and try again
        fs::remove_file(&temp_path1.join("text.txt")).await?;
        fs_utils
            .delete_parents(&temp_path1.join("text.txt"), true)
            .await?;
        assert!(!&temp_path1.exists());
        fs_utils
            .delete_parents(&temp_path2.join("text.txt"), true)
            .await?;
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
        pm.sudo_exec("chown", ["root:root", temp_path1.to_str().unwrap()], None)
            .await?;
        pm.sudo_exec("chown", ["root:root", temp_path2.to_str().unwrap()], None)
            .await?;
        pm.sudo_exec("chmod", ["600", temp_path1.to_str().unwrap()], None)
            .await?;
        pm.sudo_exec("chmod", ["600", temp_path2.to_str().unwrap()], None)
            .await?;

        // Try to delete non-empty dir
        fs_utils
            .delete_parents(&temp_path1.join("text.txt"), true)
            .await?;
        assert!(&temp_path1.exists());
        // Remove file and try again
        pm.sudo_exec(
            "rm",
            ["-f", temp_path1.join("text.txt").to_str().unwrap()],
            None,
        )
        .await?;
        fs_utils
            .delete_parents(&temp_path1.join("text.txt"), true)
            .await?;
        assert!(!&temp_path1.exists());
        fs_utils
            .delete_parents(&temp_path2.join("text.txt"), true)
            .await?;
        assert!(!&temp_path2.exists());
        // Verify that the grandparent got deleted as well
        assert!(!&temp_dir.path().exists());
        Ok(())
    }
}
