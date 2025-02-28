use std::path::PathBuf;

use serde::{Deserialize, Deserializer};

use crate::modules::conditional::Conditional;

/// Describes the configuration for a file within a module.
///
/// This includes source location, content, deployment phase, and action type, along with
/// conditional deployment logic.
#[derive(Deserialize, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ModuleFile {
    /// The source path of the file. This field is optional.
    // #[serde(default)] is necessary here so the missing field is parsed as None.
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_source")]
    pub(crate) source: Option<PathBuf>,
    /// The content of the file as a string. This allows direct specification of file content within
    /// the configuration. This field is optional.
    pub(crate) content: Option<String>,
    /// Specifies the deployment phase for the file. Defaults to "deploy".
    #[serde(default = "default_phase")]
    pub(crate) phase: Option<String>,
    /// The action to be taken with this file ("link", "copy" or "create"). Defaults to "link".
    #[serde(default = "default_action")]
    pub(crate) action: Option<String>,
    /// A conditional expression evaluated to decide if the file should be deployed.
    /// Deployment occurs only if this condition evaluates to true.
    /// This field is optional.
    pub(crate) eval_when: Option<String>,
    /// File permissions and ownership.
    pub(crate) permissions: Option<FilePermissions>,
    /// If file is a template
    #[serde(default = "default_template")]
    pub(crate) template: Option<bool>,
}

/// Provides default value for template.
fn default_template() -> Option<bool> {
    Some(false)
}
// Default values for ModuleFile
/// Provides default values for the deployment phase of a file.
fn default_phase() -> Option<String> {
    Some("deploy".to_string())
}

/// Provides default values for the deployment action of a file.
fn default_action() -> Option<String> {
    Some("link".to_string())
}

/// Custom deserializer for the `source` field in `ModuleFile`
fn deserialize_source<'de, D>(deserializer: D) -> Result<Option<PathBuf>, D::Error>
where
    D: Deserializer<'de>,
{
    let source: Option<String> = Option::deserialize(deserializer)?;
    source
        .map(|s| {
            if shellexpand::full(&s)?.starts_with('/') {
                shellexpand::full(&s).map(|expanded| PathBuf::from(expanded.as_ref()))
            } else {
                shellexpand::full(&format!(
                    "{}{}{}",
                    std::env::var("DOD_CURRENT_MODULE").expect(
                        "env variable `DOD_CURRENT_MODULE` should be set by `modules::add_module`"
                    ),
                    std::path::MAIN_SEPARATOR_STR,
                    &s
                ))
                .map(|expanded| PathBuf::from(expanded.as_ref()))
            }
        })
        .transpose()
        .map_err(serde::de::Error::custom)
}

/// File permission representation
///
/// This includes owner and group as well as access permissions
#[derive(Deserialize, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct FilePermissions {
    pub(crate) owner: Option<String>,
    pub(crate) group: Option<String>,
    pub(crate) permissions: Option<String>,
}

/// Implementation of `Conditional` for `ModuleFile`, providing access to its `eval_when` field.
impl Conditional for ModuleFile {
    fn eval_when(&self) -> &Option<String> {
        &self.eval_when
    }
}
