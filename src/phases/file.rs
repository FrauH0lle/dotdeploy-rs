use crate::modules::files::FileOperation;
use crate::store::Store;
use crate::store::sqlite::SQLiteStore;
use crate::store::sqlite_files::StoreFileBuilder;
use crate::utils::FileUtils;
use crate::utils::common::os_str_to_bytes;
use crate::utils::file_metadata::FileMetadata;
use crate::utils::file_permissions;
use crate::utils::sudo::PrivilegeManager;
use color_eyre::Result;
use color_eyre::eyre::{WrapErr, eyre};
use derive_builder::Builder;
use handlebars::Handlebars;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use toml::Value;
use tracing::{debug, info};

#[derive(Debug, Default, Clone, Deserialize, Serialize, Builder)]
#[builder(setter(prefix = "with"))]
pub(crate) struct PhaseFile {
    #[builder(setter(into))]
    pub(crate) module_name: String,
    pub(crate) source: Option<PathBuf>,
    #[builder(setter(into))]
    pub(crate) target: PathBuf,
    pub(crate) content: Option<String>,
    pub(crate) operation: FileOperation,
    pub(crate) template: bool,
    pub(crate) owner: Option<String>,
    pub(crate) group: Option<String>,
    pub(crate) permissions: Option<String>,
}

impl PhaseFile {
    pub(crate) async fn deploy(
        &self,
        pm: Arc<PrivilegeManager>,
        store: Arc<SQLiteStore>,
        context: Arc<HashMap<String, Value>>,
        hb: Arc<Handlebars<'static>>,
    ) -> Result<()> {
        // Check if the previous operation is still valid
        let cur_operation = match self.operation {
            FileOperation::Copy => "copy",
            FileOperation::Link => "link",
            FileOperation::Create => "create",
        };
        let prev_operation = match store.get_file(&self.target).await? {
            Some(f) => f.operation,
            None => cur_operation.to_string(),
        };

        if prev_operation.as_str() != cur_operation {
            info!(
                "{}: File operation changed from {} -> {}. Re-deploying",
                &self.target.display(),
                prev_operation,
                cur_operation
            );
            let file_utils = FileUtils::new(Arc::clone(&pm));
            file_utils.delete_file(&self.target).await?;

            // Restore backup, if any
            if store.check_backup_exists(&self.target).await? {
                store.restore_backup(&self.target, &self.target).await?;
                // Remove backup
                store.remove_backup(&self.target).await?;
            }

            // Remove file from store
            store.remove_file(&self.target).await?;
        }
        match self.operation {
            FileOperation::Copy => self.copy(pm, &store, &context, &hb).await?,
            FileOperation::Link => self.link(pm, &store).await?,
            FileOperation::Create => self.create(pm, &store, &context, &hb).await?,
        }

        Ok(())
    }

    async fn copy(
        &self,
        pm: Arc<PrivilegeManager>,
        store: &SQLiteStore,
        context: &HashMap<String, Value>,
        hb: &Handlebars<'static>,
    ) -> Result<()> {
        let file_utils = FileUtils::new(pm);
        let source_file = self
            .source
            .as_ref()
            .expect("A copy file cannot be without source");

        let source_file_checksum = Some(file_utils.calculate_sha256_checksum(&source_file).await?);
        let source_store_checksum = store
            .get_source_checksum(&self.target)
            .await?
            .source_checksum;
        let target_file_checksum = if file_utils.check_path_exists(&self.target).await? {
            Some(file_utils.calculate_sha256_checksum(&self.target).await?)
        } else {
            None
        };
        let target_store_checksum = store
            .get_target_checksum(&self.target)
            .await?
            .target_checksum;

        // A file should not be copied if:
        // - It is already in the store
        //   - and source checksum in store and source checksum of file match
        //   - and target checksum in store and target checksum of file match
        if source_store_checksum.as_ref() == source_file_checksum.as_ref()
            && target_store_checksum.as_ref() == target_file_checksum.as_ref()
        {
            debug!("{} deployed and up to date", self.target.display());
            return Ok(());
        }

        // Backup file
        if !store.check_backup_exists(&self.target).await? {
            debug!("Creating backup of {}", &self.target.display());
            if file_utils.check_path_exists(&self.target).await? {
                store.add_backup(&self.target).await?
            } else {
                store.add_dummy_backup(&self.target).await?
            }
        }

        debug!(
            "Trying to copy {} -> {}",
            &source_file.display(),
            &self.target.display()
        );

        if self.template {
            // If it's a template, render it to a temporary file first
            let temp_file = tempfile::NamedTempFile::new()?;
            let file_content = fs::read_to_string(source_file).await?;
            let rendered = hb
                .render_template(&file_content, context)
                .wrap_err_with(|| format!("Failed to render template {}", source_file.display()))?;
            fs::write(&temp_file, rendered).await?;

            file_utils.copy_file(temp_file.path(), &self.target).await?;
        } else {
            // Otherwise just copy the file
            file_utils.copy_file(source_file, &self.target).await?;
        }
        info!(
            "Copied {} -> {}",
            &source_file.display(),
            &self.target.display()
        );

        // Set metadata, if necessary
        file_utils
            .set_file_metadata(
                &self.target,
                FileMetadata {
                    uid: self
                        .owner
                        .as_ref()
                        .map(file_permissions::user_to_uid)
                        .transpose()?,
                    gid: self
                        .group
                        .as_ref()
                        .map(file_permissions::group_to_gid)
                        .transpose()?,
                    permissions: self
                        .permissions
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
            .add_file(
                StoreFileBuilder::default()
                    .with_module(&self.module_name)
                    .with_source(Some(source_file.to_string_lossy().to_string()))
                    .with_source_u8(Some(os_str_to_bytes(source_file)))
                    .with_source_checksum(Some(
                        file_utils.calculate_sha256_checksum(&source_file).await?,
                    ))
                    .with_target(self.target.to_string_lossy().to_string())
                    .with_target_u8(os_str_to_bytes(&self.target))
                    .with_target_checksum(Some(
                        file_utils.calculate_sha256_checksum(&self.target).await?,
                    ))
                    .with_operation("copy")
                    .with_user(Some(whoami::username()))
                    .with_date(chrono::offset::Utc::now())
                    .build()?,
            )
            .await?;

        Ok(())
    }

    async fn link(&self, pm: Arc<PrivilegeManager>, store: &SQLiteStore) -> Result<()> {
        let file_utils = FileUtils::new(pm);
        let source_file = self
            .source
            .as_ref()
            .expect("A link file cannot be without source");

        // A file should not be linked if:
        // - It is already in the store
        //   - and a link between source and target exists
        if file_utils.check_path_exists(&self.target).await?
            && file_utils
                .check_link_exists(&self.target, Some(source_file))
                .await?
            && store.check_file_exists(&self.target).await?
        {
            debug!("{} deployed and up to date", self.target.display());
            return Ok(());
        }

        // Backup file
        if !store.check_backup_exists(&self.target).await? {
            debug!("Creating backup of {}", &self.target.display());
            if file_utils.check_path_exists(&self.target).await? {
                store.add_backup(&self.target).await?
            } else {
                store.add_dummy_backup(&self.target).await?
            }
        }

        // Remove present file
        file_utils.delete_file(&self.target).await?;

        debug!(
            "Trying to link {} -> {}",
            &source_file.display(),
            &self.target.display()
        );

        file_utils.link_file(source_file, &self.target).await?;
        info!(
            "Linked {} -> {}",
            &source_file.display(),
            &self.target.display()
        );

        // Set metadata, if necessary
        file_utils
            .set_file_metadata(
                &self.target,
                FileMetadata {
                    uid: self
                        .owner
                        .as_ref()
                        .map(file_permissions::user_to_uid)
                        .transpose()?,
                    gid: self
                        .group
                        .as_ref()
                        .map(file_permissions::group_to_gid)
                        .transpose()?,
                    permissions: None,
                    is_symlink: true,
                    symlink_source: None,
                    checksum: None,
                },
            )
            .await?;

        store
            .add_file(
                StoreFileBuilder::default()
                    .with_module(&self.module_name)
                    .with_source(Some(source_file.to_string_lossy().to_string()))
                    .with_source_u8(Some(os_str_to_bytes(source_file)))
                    .with_source_checksum(Some(
                        file_utils.calculate_sha256_checksum(&source_file).await?,
                    ))
                    .with_target(self.target.to_string_lossy().to_string())
                    .with_target_u8(os_str_to_bytes(&self.target))
                    .with_target_checksum(None)
                    .with_operation("link")
                    .with_user(Some(whoami::username()))
                    .with_date(chrono::offset::Utc::now())
                    .build()?,
            )
            .await?;

        Ok(())
    }

    async fn create(
        &self,
        pm: Arc<PrivilegeManager>,
        store: &SQLiteStore,
        context: &HashMap<String, Value>,
        hb: &Handlebars<'static>,
    ) -> Result<()> {
        let file_utils = FileUtils::new(pm);

        // Ensure the parent directory exists
        let parent = self
            .target
            .parent()
            .ok_or_else(|| eyre!("Could not get parent of {}", &self.target.display()))?;
        file_utils.ensure_dir_exists(parent).await?;

        let content = self
            .content
            .as_ref()
            .expect("A create file cannot be without content");

        // Backup file
        if !store.check_backup_exists(&self.target).await? {
            debug!("Creating backup of {}", &self.target.display());
            if file_utils.check_path_exists(&self.target).await? {
                store.add_backup(&self.target).await?
            } else {
                store.add_dummy_backup(&self.target).await?
            }
        }

        debug!("Trying to create {}", &self.target.display());

        if self.template {
            // If it's a template, render it to a temporary file first
            let temp_file = tempfile::NamedTempFile::new()?;
            let rendered = hb.render_template(content, context).wrap_err_with(|| {
                format!("Failed to render template {}", temp_file.path().display())
            })?;
            fs::write(&temp_file, rendered).await?;

            file_utils.copy_file(temp_file.path(), &self.target).await?;
        } else {
            let temp_file = tempfile::NamedTempFile::new()?;
            fs::write(&temp_file, &content).await?;
            file_utils.copy_file(temp_file.path(), &self.target).await?;
        }
        info!("Created {}", &self.target.display());

        // Set metadata, if necessary
        file_utils
            .set_file_metadata(
                &self.target,
                FileMetadata {
                    uid: self
                        .owner
                        .as_ref()
                        .map(file_permissions::user_to_uid)
                        .transpose()?,
                    gid: self
                        .group
                        .as_ref()
                        .map(file_permissions::group_to_gid)
                        .transpose()?,
                    permissions: self
                        .permissions
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
            .add_file(
                StoreFileBuilder::default()
                    .with_module(&self.module_name)
                    .with_source(None)
                    .with_source_u8(None)
                    .with_source_checksum(None)
                    .with_target(self.target.to_string_lossy().to_string())
                    .with_target_u8(os_str_to_bytes(&self.target))
                    .with_target_checksum(Some(
                        file_utils.calculate_sha256_checksum(&self.target).await?,
                    ))
                    .with_operation("create")
                    .with_user(Some(whoami::username()))
                    .with_date(chrono::offset::Utc::now())
                    .build()?,
            )
            .await?;

        Ok(())
    }
}
