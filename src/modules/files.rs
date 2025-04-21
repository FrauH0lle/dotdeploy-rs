use crate::modules::{
    ConditionEvaluator, ConditionalComponent, DeployPhase,
    default_option_bool,
};
use color_eyre::eyre::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::error;

/// Represents a file to be managed by a dotdeploy module.
///
/// This struct defines the configuration for files that will be deployed, linked, or otherwise
/// managed by dotdeploy. Each file can have various properties that control how it's processed,
/// including source and destination paths, content, permissions, and conditional deployment.
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct ModuleFile {
    /// Path to the source file relative to the module directory.
    /// Not required if `content` is provided directly.
    pub(crate) source: Option<PathBuf>,
    /// Destination path where the file should be deployed.
    /// This is the only required field for a file entry.
    pub(crate) target: PathBuf,
    /// Direct content to be written to the target file.
    /// If provided, this is used instead of reading from `source`.
    pub(crate) content: Option<String>,
    /// When this file should be processed.
    /// Can be either "setup", "config" (default) or "remove".
    #[serde(default)]
    pub(crate) phase: DeployPhase,
    /// The operation to perform with this file.
    /// Possible values: "link" (default), "copy" or "create".
    #[serde(default, rename = "type")]
    pub(crate) operation: FileOperation,
    /// Conditional expression that determines if this file should be processed.
    /// If the condition evaluates to false, the file is skipped.
    #[serde(rename = "if")]
    pub(crate) condition: Option<String>,
    /// Whether to process the file as a Handlebars template.
    /// If true, the file content is rendered with the module's context variables. Only vaid for
    /// "copy" or "create" operations.
    #[serde(default = "default_option_bool")]
    pub(crate) template: Option<bool>,
    /// The user that should own the deployed file.
    /// If not specified, the current user's ownership is maintained.
    pub(crate) owner: Option<String>,
    /// The group that should own the deployed file.
    /// If not specified, the current group ownership is maintained.
    pub(crate) group: Option<String>,
    /// File permissions to set on the deployed file, in octal format (e.g., "0644").
    /// If not specified, default permissions are used.
    pub(crate) permissions: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
pub(crate) enum FileOperation {
    Copy,
    #[default]
    Link,
    Create,
}

impl ConditionEvaluator for ModuleFile {
    fn eval_condition<T>(&self, context: &T, hb: &handlebars::Handlebars<'static>) -> Result<bool>
    where
        T: Serialize,
    {
        if let Some(ref condition) = self.condition {
            Self::eval_condition_helper(condition, context, hb)
        } else {
            // Just return true if there is no condition
            Ok(true)
        }
    }
}

impl ConditionalComponent for ModuleFile {
    fn log_error(&self, module: &str, location: &Path, err: impl std::fmt::Display) {
        error!(
            module,
            location = ?location,
            target_file = self.target.display().to_string(),
            "File condition evaluation failed: {}",
            err
        );
    }
}
