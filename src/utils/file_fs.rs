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
use std::ffi::OsStr;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::warn;

/// Expands a path, resolving environment variables and tilde expressions.
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
/// Returns an error if environment variables cannot be expanded.
pub(crate) fn expand_path<P: AsRef<Path>, S: AsRef<OsStr>>(
    path: P,
    env: Option<&HashMap<String, S>>,
) -> Result<PathBuf> {
    let home_dir = || -> Option<PathBuf> { dirs::home_dir() };

    // Create variable lookup closure that checks custom env first, then system env
    let context = |var: &str| -> Result<Option<OsString>> {
        // Check custom environment variables first
        if let Some(custom_env) = env {
            if let Some(value) = custom_env.get(var) {
                return Ok(Some(value.as_ref().into()));
            }
        }
        // Fall back to system environment variables
        Ok(std::env::var_os(var))
    };

    // Expand the path using shellexpand with custom context
    let expanded = shellexpand::path::full_with_context(&path, home_dir, context)
        .map_err(|e| eyre!("Failed to expand path: {:?}", e))?;

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
                    .sudo_exec_success(
                        OsString::from("test"),
                        [OsString::from("-e"), path.as_ref().into()],
                        None,
                    )
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
                            .sudo_exec_success(
                                OsString::from("test"),
                                [OsString::from("-L"), path.as_ref().into()],
                                None,
                            )
                            .await?
                        {
                            let orig = String::from_utf8(
                                self.privilege_manager
                                    .sudo_exec_output(
                                        OsString::from("readlink"),
                                        [path.as_ref().into()],
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
                        .sudo_exec_success(
                            OsString::from("test"),
                            [OsString::from("-L"), path.as_ref().into()],
                            None,
                        )
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
                    .sudo_exec(
                        OsString::from("mkdir"),
                        [OsString::from("-p"), path.as_ref().into()],
                        None,
                    )
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
                    OsString::from("cp"),
                    [from.as_ref().into(), to.as_ref().into()],
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
                    OsString::from("ln"),
                    [
                        OsString::from("-sf"),
                        from.as_ref().into(),
                        to.as_ref().into(),
                    ],
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
    pub(crate) async fn delete_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        match fs::remove_file(&path).await {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Ok(self
                .privilege_manager
                .sudo_exec(
                    OsString::from("rm"),
                    [OsString::from("-f"), path.as_ref().into()],
                    None,
                )
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
                    let output = self
                        .privilege_manager
                        .sudo_exec_output(
                            OsString::from("find"),
                            [
                                path.into(),
                                OsString::from("-maxdepth"),
                                OsString::from("0"),
                                OsString::from("-empty"),
                            ],
                            None,
                        )
                        .await?;
                    if output.status.success() {
                        !output.stdout.is_empty()
                    } else {
                        return Err(eyre!(
                            "Failed to check if directory {} is empty: {}",
                            path.to_string_lossy(),
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
                    format_args!("Directory {} is now empty. Delete [y/N]?", path.display()),
                    "(You can skip this prompt with the CLI argument '-y true' or '--noconfirm=true')"
                ))
            {
                match fs::remove_dir(path).await {
                    Ok(_) => (),
                    Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                        // If permission is denied, use sudo to remove the directory
                        self.privilege_manager
                            .sudo_exec(OsString::from("rmdir"), [path.into()], None)
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
    fn test_expand_path() -> Result<()> {
        // Test with tilde expansion
        let home = dirs::home_dir().ok_or_eyre("Failed to get HOME dir")?;
        assert_eq!(
            expand_path::<&str, &str>("~/test.txt", None)?,
            PathBuf::from(format!("{}/test.txt", home.to_str().ok_or_eyre("Invalid UTF-8")?))
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
            PathBuf::from(format!("{}/dir/tmp/test/file.txt", home.to_str().ok_or_eyre("Invalid UTF-8")?))
        );

        // Test with absolute path (no expansion needed)
        assert_eq!(
            expand_path::<&str, &str>("/absolute/path.txt", None)?,
            PathBuf::from("/absolute/path.txt")
        );

        // Test with invalid UTF-8
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        assert!(
            expand_path::<PathBuf, &str>(PathBuf::from(OsString::from_vec(vec![255])), None)
                .is_err()
        );

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
