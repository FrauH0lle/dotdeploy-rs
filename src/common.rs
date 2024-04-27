use std::io::{stdin, stdout, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
/// Helper functions used in various places
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};
use tokio::fs;

use crate::sudo;

//
// User input

/// Ask for simple confirmation from user.
pub fn ask_boolean(prompt: &str) -> bool {
    // enter the loop at least once
    let mut buf = String::from("a");
    while !(buf.to_lowercase().starts_with('y')
        || buf.to_lowercase().starts_with('n')
        || buf.is_empty())
    {
        eprintln!("{}", prompt);
        buf.clear();
        stdout().flush().expect("Failed to flush stdout");
        stdin()
            .read_line(&mut buf)
            .expect("Failed to read line from stdin");
    }

    // If empty defaults to no
    buf.to_lowercase().starts_with('y')
}

//
// Files

/// Transform AsRef<Path> to Result<String>
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

/// Test if a file exists, elevating privileges if a PermissionDenied error is encountered.
pub(crate) async fn check_file_exists<P: AsRef<Path>>(path: P) -> Result<bool> {
    match path.as_ref().try_exists() {
        Ok(false) => Ok(false),
        Ok(true) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            Ok(sudo::sudo_exec_success("test", &["-e", &path_to_string(path)?], None).await?)
        }
        Err(e) => {
            Err(e).with_context(|| format!("Falied to check existence of {:?}", &path.as_ref()))?
        }
    }
}

/// Test if a link exists, elevating privileges if a PermissionDenied error is encountered.
/// Optionally verifies that it points to a specific source.
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

/// Delete a file, elevating privileges if a PermissionDenied error is encountered.
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
            || ask_boolean(&format!(
                "Directory at {:?} is now empty. Delete [y/N]? ",
                path
            ))
        {
            match fs::remove_dir(path).await {
                Ok(_) => (),
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
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

/// Calculate sha256 checksum and elevate privileges if necessary
pub(crate) async fn calculate_sha256_checksum<P: AsRef<Path>>(path: P) -> Result<String> {
    // Open the file asynchronously
    let checksum = match fs::read(&path).await {
        Ok(content) => {
            // Perform the hashing in a blocking task to not block the async executor
            // Await the spawned task, then propagate errors
            tokio::task::spawn_blocking(move || {
                let mut hasher = Sha256::new();
                hasher.update(&content);
                // Convert the hash to a hexadecimal string
                format!("{:x}", hasher.finalize())
            })
            .await?
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            let output = sudo::sudo_exec_output("sha256sum", &[path.as_ref()], None)
                .await?
                .stdout;

            if output.is_empty() {
                bail!("sha256sum {:?} did not return any output", path.as_ref())
            } else {
                String::from_utf8(output)?
                    .split_whitespace()
                    .next()
                    .ok_or_else(|| anyhow!("Failed to split whitespace"))?
                    .trim()
                    .to_string()
            }
        }
        Err(e) => Err(e)
            .with_context(|| format!("Falied to calculate checksum of {:?}", &path.as_ref()))?,
    };

    Ok(checksum)
}

pub(crate) fn perms_int_to_str(p: u32) -> Result<String> {
    let s = format!("{:o}", p);
    // Take only the last three digits of the conversion result
    let split_pos = s.char_indices().nth_back(2).unwrap().0;
    Ok(s[split_pos..].to_string())
}

pub(crate) fn perms_str_to_int<S: AsRef<str>>(p: S) -> Result<u32> {
    u32::from_str_radix(p.as_ref(), 8).context("Failed to convert permission string to u32")
}

pub(crate) fn user_to_uid<S: AsRef<str>>(u: S) -> Result<u32> {
    Ok(nix::unistd::User::from_name(u.as_ref())
        .unwrap()
        .unwrap()
        .uid
        .as_raw())
}

pub(crate) fn group_to_gid<S: AsRef<str>>(u: S) -> Result<u32> {
    Ok(nix::unistd::Group::from_name(u.as_ref())
        .unwrap()
        .unwrap()
        .gid
        .as_raw())
}

pub(crate) struct FileMetadata {
    pub(crate) uid: Option<u32>,
    pub(crate) gid: Option<u32>,
    pub(crate) permissions: Option<u32>,
    pub(crate) is_symlink: bool,
    pub(crate) symlink_source: Option<PathBuf>,
    pub(crate) checksum: Option<String>,
}

/// Get file metadata, elevating privileges if necessary
pub(crate) async fn get_file_metadata<P: AsRef<Path>>(path: P) -> Result<FileMetadata> {
    let metadata = match tokio::fs::symlink_metadata(&path).await {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            let temp_file = tempfile::NamedTempFile::new()?;
            let temp_path_str = path_to_string(&temp_file)?;

            sudo::sudo_exec(
                "cp",
                &[
                    "--preserve",
                    "--no-dereference",
                    &path_to_string(&path)?,
                    &temp_path_str,
                ],
                Some(
                    format!(
                        "Create temporary copy of {:?} for metadata retrieval",
                        &path.as_ref()
                    )
                    .as_str(),
                ),
            )
            .await?;

            tokio::fs::symlink_metadata(&temp_file)
                .await
                .with_context(|| format!("Failed to get metadata of {:?}", &temp_file))?
        }
        Err(e) => {
            Err(e).with_context(|| format!("Falied to get metadata of {:?}", &path.as_ref()))?
        }
    };

    Ok(FileMetadata {
        uid: Some(metadata.uid()),
        gid: Some(metadata.gid()),
        permissions: Some(metadata.mode()),
        is_symlink: metadata.is_symlink(),
        symlink_source: if metadata.is_symlink() {
            Some(fs::read_link(&path).await?)
        } else {
            None
        },
        checksum: if metadata.is_symlink() {
            None
        } else {
            Some(calculate_sha256_checksum(&path).await?)
        },
    })
}

/// Set file metadata, elevating privileges if necessary
pub(crate) async fn set_file_metadata<P: AsRef<Path>>(
    path: P,
    metadata: FileMetadata,
) -> Result<()> {
    if let Some(permissions) = metadata.permissions {
        match fs::set_permissions(&path, std::fs::Permissions::from_mode(permissions)).await {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                sudo::sudo_exec(
                    "chmod",
                    &[&perms_int_to_str(permissions)?, &path_to_string(&path)?],
                    None,
                )
                .await?
            }
            Err(e) => Err(e)
                .with_context(|| format!("Failed to set permissions for {:?}", &path.as_ref()))?,
        }
    }
    if let (Some(uid), Some(gid)) = (metadata.uid, metadata.gid) {
        match std::os::unix::fs::lchown(&path, Some(uid), Some(gid)) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                sudo::sudo_exec(
                    "chown",
                    &[format!("{}:{}", uid, gid).as_str(), &path_to_string(&path)?],
                    None,
                )
                .await?
            }
            Err(e) => Err(e).with_context(|| {
                format!("Failed to set user and group for {:?}", &path.as_ref())
            })?,
        }
    }

    Ok(())
}

//
// Tests

#[cfg(test)]
mod tests {
    use super::*;

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
        tokio::fs::File::create(&temp_file).await?;
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
