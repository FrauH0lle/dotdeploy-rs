use crate::modules::files::ModuleFile;
use crate::modules::tasks::ModuleTask;
use crate::store::Stores;
use crate::store::sqlite_files::StoreFile;
use crate::utils::FileUtils;
use crate::utils::file_fs;
use crate::utils::file_metadata::FileMetadata;
use crate::utils::file_permissions;
use crate::utils::sudo::PrivilegeManager;
use color_eyre::eyre::{WrapErr, eyre};
use color_eyre::{Report, Result, Section};
use handlebars::Handlebars;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio::task::JoinSet;
use toml::Value;
use tracing::{debug, info, instrument, warn};

#[derive(Debug, Default, Clone)]
pub(crate) enum PhaseFileOp {
    Copy,
    #[default]
    Link,
    Create,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct PhaseFile {
    pub(crate) module_name: String,
    pub(crate) source: Option<PathBuf>,
    pub(crate) target: PathBuf,
    pub(crate) content: Option<String>,
    pub(crate) operation: PhaseFileOp,
    pub(crate) template: bool,
    pub(crate) owner: Option<String>,
    pub(crate) group: Option<String>,
    pub(crate) permissions: Option<String>,
}

impl PhaseFile {
    pub(crate) async fn deploy(
        &self,
        pm: Arc<PrivilegeManager>,
        stores: Arc<Stores>,
        context: Arc<HashMap<String, Value>>,
        hb: Arc<Handlebars<'static>>,
    ) -> Result<()> {
        // Check if the previous operation is still valid
        let cur_operation = match self.operation {
            PhaseFileOp::Copy => "copy",
            PhaseFileOp::Link => "link",
            PhaseFileOp::Create => "create",
        };
        // FIXME 2025-03-20: This should be handled better by get_file
        let prev_operation = if stores.check_file_exists(&self.target).await? {
            stores.get_file(&self.target).await?.operation
        } else {
            cur_operation.to_string()
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
            if stores.check_backup_exists(&self.target).await? {
                stores.restore_backup(&self.target, &self.target).await?;
                // Remove backup
                stores.remove_backup(&self.target).await?;
            }

            // Remove file from store
            stores
                .remove_file(file_fs::path_to_string(&self.target)?)
                .await?;
        }
        match self.operation {
            PhaseFileOp::Copy => self.copy(pm, &*stores, &*context, &*hb).await?,
            PhaseFileOp::Link => self.link(pm, &*stores).await?,
            PhaseFileOp::Create => self.create(pm, &*stores, &*context, &*hb).await?,
        }

        Ok(())
    }

    async fn copy(
        &self,
        pm: Arc<PrivilegeManager>,
        stores: &Stores,
        context: &HashMap<String, Value>,
        hb: &Handlebars<'static>,
    ) -> Result<()> {
        let file_utils = FileUtils::new(pm);
        let source_file = self
            .source
            .as_ref()
            .expect("A copy file cannot be without source");

        let source_file_checksum = Some(file_utils.calculate_sha256_checksum(&source_file).await?);
        let source_store_checksum = stores
            .get_source_checksum(&self.target)
            .await?
            .source_checksum;
        let target_file_checksum = if file_utils.check_file_exists(&self.target).await? {
            Some(file_utils.calculate_sha256_checksum(&self.target).await?)
        } else {
            None
        };
        let target_store_checksum = stores
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
        if !stores.check_backup_exists(&self.target).await? {
            debug!("Creating backup of {}", &self.target.display());
            if file_utils.check_file_exists(&self.target).await? {
                stores.add_backup(&self.target).await?
            } else {
                stores.add_dummy_backup(&self.target).await?
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
                        .map(|o| file_permissions::user_to_uid(o))
                        .transpose()?,
                    gid: self
                        .group
                        .as_ref()
                        .map(|g| file_permissions::group_to_gid(g))
                        .transpose()?,
                    permissions: self
                        .permissions
                        .as_ref()
                        .map(|p| file_permissions::perms_str_to_int(p))
                        .transpose()?,
                    is_symlink: false,
                    symlink_source: None,
                    checksum: None,
                },
            )
            .await?;

        stores
            .add_file(StoreFile::new(
                self.module_name.clone(),
                Some(file_fs::path_to_string(&source_file)?),
                Some(file_utils.calculate_sha256_checksum(&source_file).await?),
                file_fs::path_to_string(&self.target)?,
                Some(file_utils.calculate_sha256_checksum(&self.target).await?),
                "copy".to_string(),
                Some(whoami::username()),
                chrono::offset::Utc::now(),
            ))
            .await?;

        Ok(())
    }

    async fn link(&self, pm: Arc<PrivilegeManager>, stores: &Stores) -> Result<()> {
        let file_utils = FileUtils::new(pm);
        let source_file = self
            .source
            .as_ref()
            .expect("A link file cannot be without source");

        // A file should not be linked if:
        // - It is already in the store
        //   - and a link between source and target exists
        if file_utils.check_file_exists(&self.target).await?
            && file_utils
                .check_link_exists(&self.target, Some(source_file))
                .await?
            && stores.check_file_exists(&self.target).await?
        {
            debug!("{} deployed and up to date", self.target.display());
            return Ok(());
        }

        // Backup file
        if !stores.check_backup_exists(&self.target).await? {
            debug!("Creating backup of {}", &self.target.display());
            if file_utils.check_file_exists(&self.target).await? {
                stores.add_backup(&self.target).await?
            } else {
                stores.add_dummy_backup(&self.target).await?
            }
        }

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
                        .map(|o| file_permissions::user_to_uid(o))
                        .transpose()?,
                    gid: self
                        .group
                        .as_ref()
                        .map(|g| file_permissions::group_to_gid(g))
                        .transpose()?,
                    permissions: None,
                    is_symlink: true,
                    symlink_source: None,
                    checksum: None,
                },
            )
            .await?;

        stores
            .add_file(StoreFile::new(
                self.module_name.clone(),
                Some(file_fs::path_to_string(&source_file)?),
                Some(file_utils.calculate_sha256_checksum(&source_file).await?),
                file_fs::path_to_string(&self.target)?,
                None,
                "link".to_string(),
                Some(whoami::username()),
                chrono::offset::Utc::now(),
            ))
            .await?;

        Ok(())
    }

    async fn create(
        &self,
        pm: Arc<PrivilegeManager>,
        stores: &Stores,
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
        if !stores.check_backup_exists(&self.target).await? {
            debug!("Creating backup of {}", &self.target.display());
            if file_utils.check_file_exists(&self.target).await? {
                stores.add_backup(&self.target).await?
            } else {
                stores.add_dummy_backup(&self.target).await?
            }
        }

        debug!("Trying to create {}", &self.target.display());

        if self.template {
            // If it's a template, render it to a temporary file first
            let temp_file = tempfile::NamedTempFile::new()?;
            let rendered = hb.render_template(&content, context).wrap_err_with(|| {
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
                        .map(|o| file_permissions::user_to_uid(o))
                        .transpose()?,
                    gid: self
                        .group
                        .as_ref()
                        .map(|g| file_permissions::group_to_gid(g))
                        .transpose()?,
                    permissions: self
                        .permissions
                        .as_ref()
                        .map(|p| file_permissions::perms_str_to_int(p))
                        .transpose()?,
                    is_symlink: false,
                    symlink_source: None,
                    checksum: None,
                },
            )
            .await?;

        stores
            .add_file(StoreFile::new(
                self.module_name.clone(),
                None,
                None,
                file_fs::path_to_string(&self.target)?,
                Some(file_utils.calculate_sha256_checksum(&self.target).await?),
                "create".to_string(),
                Some(whoami::username()),
                chrono::offset::Utc::now(),
            ))
            .await?;

        Ok(())
    }
}
