//! File operations module.
//!
//! This module contains structures and functions for performing various file operations during the
//! deployment process, such as copying, symlinking, and creating files.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use handlebars::Handlebars;
use serde_json::Value;

use crate::phases::destination::Destination;
use crate::utils::file_metadata;
use crate::utils::file_permissions;

/// Represents the type of operation to be performed on the file.
#[derive(Debug, Clone)]
pub(crate) enum FileOperation {
    /// Copy file from source to destination.
    Copy {
        source: PathBuf,
        destination: Destination,
        owner: Option<String>,
        group: Option<String>,
        permissions: Option<String>,
        template: Option<bool>,
    },
    /// Link file from source to destination.
    Symlink {
        source: PathBuf,
        destination: Destination,
        owner: Option<String>,
        group: Option<String>,
    },
    /// Create file with content at destination.
    Create {
        content: String,
        destination: Destination,
        owner: Option<String>,
        group: Option<String>,
        permissions: Option<String>,
        template: Option<bool>,
    },
}

impl FileOperation {
    /// Run the file operation appropriate for the variant.
    ///
    /// This method executes the specific file operation based on the enum variant, handling
    /// copying, symlinking, or creating files as needed.
    ///
    /// # Arguments
    ///
    /// * `context` - A JSON Value containing context for template rendering.
    /// * `hb` - A reference to a Handlebars instance for template rendering.
    ///
    /// # Returns
    ///
    /// A Result indicating success or failure of the operation.
    async fn run(&self, context: &Value, hb: &Handlebars<'static>) -> Result<()> {
        match self {
            FileOperation::Copy {
                source,
                destination,
                owner,
                group,
                permissions,
                template,
            } => {
                // Copy the file, potentially rendering it as a template
                destination.copy(source, *template, context, hb).await?;
                // Set permissions on the copied file
                self.set_permissions(destination.path(), owner, group, permissions)
                    .await?;
            }
            FileOperation::Symlink {
                source,
                destination,
                owner,
                group,
            } => {
                // Create a symbolic link
                destination.link(source).await?;
                // Set permissions on the symlink (note: permissions are None for symlinks)
                self.set_permissions(destination.path(), owner, group, &None)
                    .await?;
            }
            FileOperation::Create {
                content,
                destination,
                owner,
                group,
                permissions,
                template,
            } => {
                // Create a new file with the given content, potentially rendering it as a template
                destination.create(content, *template, context, hb).await?;
                // Set permissions on the newly created file
                self.set_permissions(destination.path(), owner, group, permissions)
                    .await?;
            }
        }
        Ok(())
    }

    /// Set the file or link permissions.
    ///
    /// This method applies the specified ownership and permissions to the given path.
    ///
    /// # Arguments
    ///
    /// * `path` - The path to the file or link.
    /// * `owner` - Optional owner (username) to set.
    /// * `group` - Optional group to set.
    /// * `permissions` - Optional permissions to set (as a string, e.g., "644").
    ///
    /// # Returns
    ///
    /// A Result indicating success or failure of setting permissions.
    async fn set_permissions(
        &self,
        path: &Path,
        owner: &Option<String>,
        group: &Option<String>,
        permissions: &Option<String>,
    ) -> Result<()> {
        file_metadata::set_file_metadata(
            path,
            file_metadata::FileMetadata {
                // Convert username to UID if owner is specified
                uid: owner.as_ref().map(|o| file_permissions::user_to_uid(o)).transpose()?,
                // Convert group name to GID if group is specified
                gid: group
                    .as_ref()
                    .map(|g| file_permissions::group_to_gid(g))
                    .transpose()?,
                // Convert permission string to numeric mode if specified
                permissions: permissions
                    .as_ref()
                    .map(|p| file_permissions::perms_str_to_int(p))
                    .transpose()?,
                is_symlink: false,
                symlink_source: None,
                checksum: None,
            },
        )
        .await
        .with_context(|| format!("Failed to set file permissions for {:?}", path))?;
        Ok(())
    }
}
