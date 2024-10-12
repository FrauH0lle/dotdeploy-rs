//! File operations module.
//!
//! This module contains structures and functions for performing various file operations during the
//! deployment process, such as copying, symlinking, and creating files.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use handlebars::Handlebars;
use serde_json::Value;

use crate::phases::destination::Destination;
use crate::Stores;
use crate::utils::file_checksum;
use crate::utils::file_fs;
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

/// A structure to manage file configurations, including the operation, source and destination.
#[derive(Debug, Clone)]
pub(crate) struct ManagedFile {
    /// Module the file belongs to
    pub(crate) module: String,
    /// Which [FileOperation] to apply.
    pub(crate) operation: FileOperation,
}

impl ManagedFile {
    pub(crate) async fn perform(
        &self,
        stores: &Stores,
        context: &serde_json::Value,
        hb: &handlebars::Handlebars<'static>,
    ) -> Result<()> {
        match &self.operation {
            FileOperation::Copy {
                source,
                destination,
                owner,
                group,
                permissions,
                template,
            } => {
                let store = match destination {
                    Destination::Home(_) => &stores.user_store,
                    Destination::Root(_) => {
                        stores.system_store.as_ref().expect("System store should not be empty")
                    }
                };

                // Perform copy operation

                // Copy when
                // - source has changed
                // - file not found in DB

                let mut do_copy = false;

                if template.expect("template should always be Some()") {
                    // Always copy the file if it is a template, no further checks
                    do_copy = true;
                } else {
                    // Check if source has changed
                    if let Some(db_src_checksum) = store
                        .get_source_checksum(destination.path())
                        .await
                        .map_err(|e| e.into_anyhow())
                        .with_context(|| {
                            format!(
                                "Failed to get source checksum for {:?} from store",
                                &destination.path()
                            )
                        })?
                    {
                        let src_checksum = file_checksum::calculate_sha256_checksum(&db_src_checksum.0)
                            .await
                            .with_context(|| {
                                format!(
                                    "Failed to get source checksum for {:?}",
                                    &db_src_checksum.0
                                )
                            })?;
                        if src_checksum != db_src_checksum.1 {
                            info!("'{}' has changed, re-deplyoing", &db_src_checksum.0);
                            do_copy = true;
                        }
                    } else {
                        info!(
                            "'{}' not found in store, deplyoing",
                            destination.path().display()
                        );
                        do_copy = true;
                    }
                }

                if do_copy {
                    // Create backup if no backup is already stored and if the destination file
                    // already exists
                    if !store
                        .check_backup_exists(destination.path())
                        .await
                        .map_err(|e| e.into_anyhow())?
                        & file_fs::check_file_exists(destination.path()).await?
                    {
                        store
                            .add_backup(destination.path())
                            .await
                            .map_err(|e| e.into_anyhow())?;
                    }
                    debug!("Trying to copy {:?} to {:?}", source, destination.path());

                    destination
                        .copy(source, *template, context, hb)
                        .await
                        .with_context(|| {
                            format!("Failed to copy {:?} to {:?}", source, destination.path())
                        })?;

                    // Set permissions
                    file_metadata::set_file_metadata(
                        destination.path(),
                        file_metadata::FileMetadata {
                            uid: owner.as_ref().map(file_permissions::user_to_uid).transpose()?,
                            gid: group.as_ref().map(file_permissions::group_to_gid).transpose()?,
                            permissions: permissions
                                .as_ref()
                                .map(file_permissions::perms_str_to_int)
                                .transpose()?,
                            is_symlink: false,
                            symlink_source: None,
                            checksum: None,
                        },
                    )
                    .await?;

                    // Record file in store
                    store
                        .add_file(crate::store::files::StoreFile {
                            module: self.module.clone(),
                            source: Some(source.display().to_string()),
                            source_checksum: Some(file_checksum::calculate_sha256_checksum(source).await?),
                            destination: destination.path().display().to_string(),
                            destination_checksum: Some(
                                file_checksum::calculate_sha256_checksum(destination.path()).await?,
                            ),
                            operation: "copy".to_string(),
                            user: Some(std::env::var("USER")?),
                            date: chrono::offset::Local::now(),
                        })
                        .await
                        .map_err(|e| e.into_anyhow())?;

                    info!(
                        "Copy: '{}' -> '{}'",
                        source.display(),
                        destination.path().display()
                    );
                } else {
                    info!("'{}' deployed and up to date", destination.path().display());
                }
            }
            FileOperation::Symlink {
                source,
                destination,
                owner,
                group,
            } => {
                let store = match destination {
                    Destination::Home(_) => &stores.user_store,
                    Destination::Root(_) => {
                        stores.system_store.as_ref().expect("System store should not be empty")
                    }
                };

                // Perform symlink operation
                if file_fs::check_file_exists(destination.path()).await?
                    && file_fs::check_link_exists(destination.path(), Some(source)).await?
                    && store
                        .check_file_exists(destination.path())
                        .await
                        .map_err(|e| e.into_anyhow())?
                {
                    info!("'{}' deployed and up to date", destination.path().display());
                } else {
                    if !store
                        .check_backup_exists(destination.path())
                        .await
                        .map_err(|e| e.into_anyhow())?
                        & file_fs::check_file_exists(destination.path()).await?
                    {
                        store
                            .add_backup(destination.path())
                            .await
                            .map_err(|e| e.into_anyhow())?;
                    }

                    debug!("Trying to link {:?} to {:?}", source, destination.path());

                    destination
                        .link(source.to_path_buf())
                        .await
                        .with_context(|| {
                            format!("Failed to link {:?} to {:?}", source, destination.path())
                        })?;

                    // Set permissions
                    file_metadata::set_file_metadata(
                        destination.path(),
                        file_metadata::FileMetadata {
                            uid: owner.as_ref().map(file_permissions::user_to_uid).transpose()?,
                            gid: group.as_ref().map(file_permissions::group_to_gid).transpose()?,
                            permissions: None,
                            is_symlink: true,
                            symlink_source: None,
                            checksum: None,
                        },
                    )
                    .await?;

                    store
                        .add_file(crate::store::files::StoreFile {
                            module: self.module.clone(),
                            source: Some(source.display().to_string()),
                            source_checksum: Some(file_checksum::calculate_sha256_checksum(source).await?),
                            destination: destination.path().display().to_string(),
                            destination_checksum: None,
                            operation: "link".to_string(),
                            user: Some(std::env::var("USER")?),
                            date: chrono::offset::Local::now(),
                        })
                        .await
                        .map_err(|e| e.into_anyhow())?;

                    info!(
                        "Link: '{}' -> '{}'",
                        source.display(),
                        destination.path().display()
                    );
                }
            }
            FileOperation::Create {
                content,
                destination,
                owner,
                group,
                permissions,
                template,
            } => {
                let store = match destination {
                    Destination::Home(_) => &stores.user_store,
                    Destination::Root(_) => {
                        stores.system_store.as_ref().expect("System store should not be empty")
                    }
                };
                // Perform create operation
                debug!(
                    "Trying to create {:?} with specified content",
                    destination.path()
                );

                if !store
                    .check_backup_exists(destination.path())
                    .await
                    .map_err(|e| e.into_anyhow())?
                    & file_fs::check_file_exists(destination.path()).await?
                {
                    store
                        .add_backup(destination.path())
                        .await
                        .map_err(|e| e.into_anyhow())?;
                }

                destination
                    .create(content, *template, context, hb)
                    .await?;

                file_metadata::set_file_metadata(
                    destination.path(),
                    file_metadata::FileMetadata {
                        uid: owner.as_ref().map(file_permissions::user_to_uid).transpose()?,
                        gid: group.as_ref().map(file_permissions::group_to_gid).transpose()?,
                        permissions: permissions
                            .as_ref()
                            .map(file_permissions::perms_str_to_int)
                            .transpose()?,
                        is_symlink: false,
                        symlink_source: None,
                        checksum: None,
                    },
                )
                .await?;

                store
                    .add_file(crate::store::files::StoreFile {
                        module: self.module.clone(),
                        source: None,
                        source_checksum: None,
                        destination: destination.path().display().to_string(),
                        destination_checksum: Some(
                            file_checksum::calculate_sha256_checksum(destination.path()).await?,
                        ),
                        operation: "create".to_string(),
                        user: Some(std::env::var("USER")?),
                        date: chrono::offset::Local::now(),
                    })
                    .await
                    .map_err(|e| e.into_anyhow())?;

                info!("Create: '{}'", destination.path().display());
            }
        };
        Ok(())
    }
}
