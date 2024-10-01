//! Helper functions handling the file destination and operations.
//!
//! This module provides functionality for managing file operations at different
//! destinations, handling both user home directory and root-owned locations.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use handlebars::Handlebars;
use serde_json::Value;
use tokio::fs;

use crate::utils::file_fs;
use crate::utils::sudo;

/// Represents the destination of the file operation, including whether sudo is required.
#[derive(Debug, Clone)]
pub(crate) enum Destination {
    /// Represents a destination in the user's home directory. Should not require sudo.
    Home(PathBuf),
    /// Represents a destination in a root-owned directory. Should require sudo.
    Root(PathBuf),
}

impl Destination {
    /// Returns a reference to the inner PathBuf.
    ///
    /// This method provides access to the actual path regardless of the variant.
    pub(crate) fn path(&self) -> &PathBuf {
        match self {
            Destination::Home(path) | Destination::Root(path) => path,
        }
    }

    /// Copies a file to the destination, with optional templating.
    ///
    /// # Arguments
    ///
    /// * `source` - The source path of the file to be copied.
    /// * `template` - Whether the file should be processed as a template.
    /// * `context` - The context for template rendering.
    /// * `hb` - The Handlebars instance for template rendering.
    ///
    /// # Returns
    ///
    /// A Result indicating success or failure of the copy operation.
    pub(crate) async fn copy<P: AsRef<Path>>(
        &self,
        source: P,
        template: Option<bool>,
        context: &Value,
        hb: &Handlebars<'static>,
    ) -> Result<()> {
        match self {
            Destination::Home(dest) => {
                self.copy_fn(source, dest, template, context, hb, false)
                    .await
            }
            Destination::Root(dest) => {
                self.copy_fn(source, dest, template, context, hb, true)
                    .await
            }
        }
    }

    /// Copies a file to the directory given by `dest`.
    async fn copy_fn<P: AsRef<Path>>(
        &self,
        source: P,
        dest: &Path,
        template: Option<bool>,
        context: &Value,
        hb: &Handlebars<'static>,
        sudo: bool,
    ) -> Result<()> {
        // Ensure the parent directory exists
        let parent = dest
            .parent()
            .ok_or_else(|| anyhow!("Could not get parent of {:?}", dest))?;
        file_fs::ensure_dir_exists(parent).await?;

        // Remove existing file if it exists
        if file_fs::check_file_exists(dest).await? {
            file_fs::delete_file(dest).await?
        }

        if template.is_some_and(|t| t == true) {
            match sudo {
                true => {
                    // If it's a template, render it to a temporary file first
                    let temp_file = tempfile::NamedTempFile::new()?;
                    let file_content = fs::read_to_string(&source).await?;
                    let rendered =
                        hb.render_template(&file_content, context)
                            .with_context(|| {
                                format!("Failed to render template {:?}", &source.as_ref())
                            })?;
                    fs::write(&temp_file, rendered).await?;

                    // Copy the temporary file to the destination using sudo
                    sudo::sudo_exec(
                        "cp",
                        &[
                            &file_fs::path_to_string(&temp_file)?,
                            &file_fs::path_to_string(dest)?,
                        ],
                        Some(&format!("Copy {:?} to {:?}", source.as_ref(), dest)),
                    )
                    .await?;
                }
                false => {
                    // If it's a template, render it before writing
                    let file_content = fs::read_to_string(&source).await?;
                    let rendered =
                        hb.render_template(&file_content, context)
                            .with_context(|| {
                                format!("Failed to render template {:?}", &source.as_ref())
                            })?;
                    fs::write(dest, rendered).await?;
                }
            }
        } else {
            match sudo {
                true => {
                    // If it's not a template, perform a simple copy using sudo
                    sudo::sudo_exec(
                        "cp",
                        &[
                            &file_fs::path_to_string(&source)?,
                            &file_fs::path_to_string(dest)?,
                        ],
                        Some(&format!("Copy {:?} to {:?}", source.as_ref(), dest)),
                    )
                    .await?;
                }
                false => {
                    // If it's not a template, perform a simple copy
                    fs::copy(&source, dest).await.with_context(|| {
                        format!("Failed to copy {:?} to {:?}", source.as_ref(), dest)
                    })?;
                }
            }
        }
        Ok(())
    }

    /// Creates a symlink at the destination pointing to the source.
    ///
    /// # Arguments
    ///
    /// * `source` - The path that the symlink should point to.
    ///
    /// # Returns
    ///
    /// A Result indicating success or failure of the symlink creation.
    pub(crate) async fn link<P: AsRef<Path>>(&self, source: P) -> Result<()> {
        match self {
            Destination::Home(dest) => self.link_fn(source, dest, false).await,
            Destination::Root(dest) => self.link_fn(source, dest, true).await,
        }
    }

    /// Creates a symlink in the directory given by `dest`.
    async fn link_fn<P: AsRef<Path>>(&self, source: P, dest: &Path, sudo: bool) -> Result<()> {
        // Ensure the parent directory exists
        let parent = dest
            .parent()
            .ok_or_else(|| anyhow!("Could not get parent of {:?}", dest))?;
        file_fs::ensure_dir_exists(parent).await?;

        // Remove existing file or symlink if it exists
        if file_fs::check_file_exists(dest).await? {
            file_fs::delete_file(dest).await?
        }

        match sudo {
            true => {
                // Create the symlink using sudo
                sudo::sudo_exec(
                    "ln",
                    &[
                        "-sf",
                        &file_fs::path_to_string(&source)?,
                        &file_fs::path_to_string(dest)?,
                    ],
                    Some(&format!("Link {:?} to {:?}", source.as_ref(), dest)),
                )
                .await?;
            }
            false => {
                // Create the symlink
                fs::symlink(&source, dest).await.with_context(|| {
                    format!("Failed to link {:?} to {:?}", source.as_ref(), dest)
                })?;
            }
        }

        Ok(())
    }

    /// Creates a file at the destination with the given content, with optional templating.
    ///
    /// # Arguments
    ///
    /// * `content` - The content to write to the file.
    /// * `template` - Whether the content should be processed as a template.
    /// * `context` - The context for template rendering.
    /// * `hb` - The Handlebars instance for template rendering.
    ///
    /// # Returns
    ///
    /// A Result indicating success or failure of the file creation.
    pub(crate) async fn create<S: AsRef<str>>(
        &self,
        content: S,
        template: Option<bool>,
        context: &Value,
        hb: &Handlebars<'static>,
    ) -> Result<()> {
        match self {
            Destination::Home(dest) => {
                self.create_fn(content, dest, template, context, hb, false)
                    .await
            }
            Destination::Root(dest) => {
                self.create_fn(content, dest, template, context, hb, true)
                    .await
            }
        }
    }

    /// Creates a file in the directory given by `dest`.
    async fn create_fn<S: AsRef<str>>(
        &self,
        content: S,
        dest: &Path,
        template: Option<bool>,
        context: &Value,
        hb: &Handlebars<'static>,
        sudo: bool,
    ) -> Result<()> {
        // Ensure the parent directory exists
        let parent = dest
            .parent()
            .ok_or_else(|| anyhow!("Could not get parent of {:?}", dest))?;
        file_fs::ensure_dir_exists(parent).await?;

        let temp_file = tempfile::NamedTempFile::new()?;

        if template.is_some_and(|t| t == true) {
            match sudo {
                true => {
                    // If it's a template, render it before writing to the temporary file
                    fs::write(
                        &temp_file,
                        hb.render_template(content.as_ref(), context)
                            .with_context(|| {
                                format!("Failed to render template for {:?}", temp_file)
                            })?,
                    )
                    .await
                    .with_context(|| format!("Failed to create {:?}", temp_file))?;
                }
                false => {
                    // If it's a template, render it before writing
                    fs::write(
                        dest,
                        hb.render_template(content.as_ref(), context)
                            .with_context(|| format!("Failed to render template for {:?}", dest))?,
                    )
                    .await
                    .with_context(|| format!("Failed to create {:?}", dest))?;
                }
            }
        } else {
            match sudo {
                true => {
                    // If it's not a template, write the content directly to the temporary file
                    fs::write(&temp_file, content.as_ref())
                        .await
                        .with_context(|| format!("Failed to create {:?}", temp_file))?;
                }
                false => {
                    // If it's not a template, write the content directly
                    fs::write(dest, content.as_ref())
                        .await
                        .with_context(|| format!("Failed to create {:?}", dest))?;
                }
            }
        }

        if sudo {
            // Copy the temporary file to the destination using sudo
            sudo::sudo_exec(
                "cp",
                &[
                    &file_fs::path_to_string(&temp_file)?,
                    &file_fs::path_to_string(dest)?,
                ],
                None,
            )
            .await?;
        }

        Ok(())
    }
}

//
// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // Helper function to create a test Handlebars instance
    fn create_test_handlebars() -> Handlebars<'static> {
        let mut hb = Handlebars::new();
        hb.set_strict_mode(true);
        hb
    }

    #[tokio::test]
    async fn test_destination_path() {
        let home_dest = Destination::Home(PathBuf::from("/home/user/file.txt"));
        let root_dest = Destination::Root(PathBuf::from("/etc/file.txt"));

        assert_eq!(home_dest.path(), &PathBuf::from("/home/user/file.txt"));
        assert_eq!(root_dest.path(), &PathBuf::from("/etc/file.txt"));
    }

    #[tokio::test]
    async fn test_copy() -> Result<()> {
        crate::USE_SUDO.store(true, std::sync::atomic::Ordering::Relaxed);

        let temp_dir = tempdir()?;
        let source_path = temp_dir.path().join("source.txt");
        let dest_path = temp_dir.path().join("dest.txt");

        fs::write(&source_path, "Hello, World!").await?;

        let home_dest = Destination::Home(dest_path.clone());
        let context = serde_json::json!({});
        let hb = create_test_handlebars();

        home_dest.copy(&source_path, None, &context, &hb).await?;

        let content = fs::read_to_string(&dest_path).await?;
        assert_eq!(content, "Hello, World!");

        // Test with elevated permissions
        file_fs::delete_file(&dest_path).await?;
        let dest_dir = dest_path
            .parent()
            .ok_or_else(|| anyhow!("Could not get parent of {:?}", dest_path))?;
        sudo::sudo_exec("chown", &["root:root", &dest_dir.to_str().unwrap()], None).await?;
        sudo::sudo_exec("chmod", &["644", &dest_dir.to_str().unwrap()], None).await?;
        let root_dest = Destination::Root(dest_path.clone());
        root_dest.copy(&source_path, None, &context, &hb).await?;

        let content = fs::read_to_string(&dest_path).await?;
        assert_eq!(content, "Hello, World!");

        Ok(())
    }

    #[tokio::test]
    async fn test_copy_with_template() -> Result<()> {
        crate::USE_SUDO.store(true, std::sync::atomic::Ordering::Relaxed);

        let temp_dir = tempdir()?;
        let source_path = temp_dir.path().join("source.txt");
        let dest_path = temp_dir.path().join("dest.txt");

        fs::write(&source_path, "Hello, {{name}}!").await?;

        let home_dest = Destination::Home(dest_path.clone());
        let context = serde_json::json!({"name": "Rust"});
        let hb = create_test_handlebars();

        home_dest
            .copy(&source_path, Some(true), &context, &hb)
            .await?;

        let content = fs::read_to_string(&dest_path).await?;
        assert_eq!(content, "Hello, Rust!");

        // Test with elevated permissions
        file_fs::delete_file(&dest_path).await?;
        let dest_dir = dest_path
            .parent()
            .ok_or_else(|| anyhow!("Could not get parent of {:?}", dest_path))?;
        sudo::sudo_exec("chown", &["root:root", &dest_dir.to_str().unwrap()], None).await?;
        sudo::sudo_exec("chmod", &["644", &dest_dir.to_str().unwrap()], None).await?;
        let root_dest = Destination::Root(dest_path.clone());
        root_dest.copy(&source_path, None, &context, &hb).await?;

        let content = fs::read_to_string(&dest_path).await?;
        assert_eq!(content, "Hello, Rust!");

        Ok(())
    }

    #[tokio::test]
    async fn test_link() -> Result<()> {
        crate::USE_SUDO.store(true, std::sync::atomic::Ordering::Relaxed);

        let temp_dir = tempdir()?;
        let source_path = temp_dir.path().join("source.txt");
        let dest_path = temp_dir.path().join("link.txt");

        fs::write(&source_path, "Hello, World!").await?;

        let home_dest = Destination::Home(dest_path.clone());
        home_dest.link(&source_path).await?;

        assert!(file_fs::check_link_exists(&dest_path, Some(&source_path)).await?);

        // Test with elevated permissions
        file_fs::delete_file(&dest_path).await?;
        let dest_dir = dest_path
            .parent()
            .ok_or_else(|| anyhow!("Could not get parent of {:?}", dest_path))?;
        sudo::sudo_exec("chown", &["root:root", &dest_dir.to_str().unwrap()], None).await?;
        sudo::sudo_exec("chmod", &["644", &dest_dir.to_str().unwrap()], None).await?;
        let root_dest = Destination::Root(dest_path.clone());
        root_dest.link(&source_path).await?;

        assert!(file_fs::check_link_exists(&dest_path, Some(&source_path)).await?);

        Ok(())
    }

    #[tokio::test]
    async fn test_create() -> Result<()> {
        crate::USE_SUDO.store(true, std::sync::atomic::Ordering::Relaxed);

        let temp_dir = tempdir()?;
        let dest_path = temp_dir.path().join("created.txt");

        let home_dest = Destination::Home(dest_path.clone());
        let context = serde_json::json!({});
        let hb = create_test_handlebars();

        home_dest
            .create("Hello, World!", None, &context, &hb)
            .await?;

        let content = fs::read_to_string(&dest_path).await?;
        assert_eq!(content, "Hello, World!");

        // Test with elevated permissions
        file_fs::delete_file(&dest_path).await?;
        let dest_dir = dest_path
            .parent()
            .ok_or_else(|| anyhow!("Could not get parent of {:?}", dest_path))?;
        sudo::sudo_exec("chown", &["root:root", &dest_dir.to_str().unwrap()], None).await?;
        sudo::sudo_exec("chmod", &["644", &dest_dir.to_str().unwrap()], None).await?;
        let root_dest = Destination::Root(dest_path.clone());
        root_dest
            .create("Hello, World!", None, &context, &hb)
            .await?;

        let content = fs::read_to_string(&dest_path).await?;
        assert_eq!(content, "Hello, World!");

        Ok(())
    }

    #[tokio::test]
    async fn test_create_with_template() -> Result<()> {
        crate::USE_SUDO.store(true, std::sync::atomic::Ordering::Relaxed);

        let temp_dir = tempdir()?;
        let dest_path = temp_dir.path().join("created.txt");

        let home_dest = Destination::Home(dest_path.clone());
        let context = serde_json::json!({"name": "Rust"});
        let hb = create_test_handlebars();

        home_dest
            .create("Hello, {{name}}!", Some(true), &context, &hb)
            .await?;

        let content = fs::read_to_string(&dest_path).await?;
        assert_eq!(content, "Hello, Rust!");

        // Test with elevated permissions
        file_fs::delete_file(&dest_path).await?;
        let dest_dir = dest_path
            .parent()
            .ok_or_else(|| anyhow!("Could not get parent of {:?}", dest_path))?;
        sudo::sudo_exec("chown", &["root:root", &dest_dir.to_str().unwrap()], None).await?;
        sudo::sudo_exec("chmod", &["644", &dest_dir.to_str().unwrap()], None).await?;
        let root_dest = Destination::Root(dest_path.clone());
        root_dest
            .create("Hello, {{name}}!", Some(true), &context, &hb)
            .await?;

        let content = fs::read_to_string(&dest_path).await?;
        assert_eq!(content, "Hello, Rust!");

        Ok(())
    }
}
