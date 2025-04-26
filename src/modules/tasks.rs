use crate::modules::{
    ConditionEvaluator, ConditionalComponent, default_option_bool, default_phase_hook,
};
use color_eyre::eyre::Result;
use serde::{Deserialize, Serialize};
use std::{ffi::OsString, path::Path};
use tracing::error;

#[derive(Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct ModuleTask {
    #[serde(default)]
    pub(crate) setup: Vec<TaskDefinition>,
    #[serde(default)]
    pub(crate) config: Vec<TaskDefinition>,
    #[serde(default)]
    pub(crate) update: Vec<TaskDefinition>,
    #[serde(default)]
    pub(crate) remove: Vec<TaskDefinition>,
    #[serde(default)]
    pub(crate) description: Option<String>,
    #[serde(rename = "if")]
    pub(crate) condition: Option<String>,
}
#[derive(Debug, Default, Deserialize, Serialize)]
struct TaskDefinitionIntermediate {
    description: Option<String>,
    shell: Option<String>,
    exec: Option<String>,
    args: Option<Vec<String>>,
    #[serde(default = "default_expand_args")]
    expand_args: Option<bool>,
    #[serde(default = "default_option_bool")]
    sudo: Option<bool>,
    #[serde(default = "default_phase_hook")]
    hook: Option<String>,
    #[serde(rename = "if")]
    condition: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
#[serde(from = "TaskDefinitionIntermediate")]
pub(crate) struct TaskDefinition {
    pub(crate) description: Option<String>,
    pub(crate) shell: Option<OsString>,
    pub(crate) exec: Option<OsString>,
    pub(crate) args: Option<Vec<OsString>>,
    #[serde(default = "default_expand_args")]
    pub(crate) expand_args: Option<bool>,
    #[serde(default = "default_option_bool")]
    pub(crate) sudo: Option<bool>,
    #[serde(default = "default_phase_hook")]
    pub(crate) hook: Option<String>,
    #[serde(rename = "if")]
    pub(crate) condition: Option<String>,
}

fn default_expand_args() -> Option<bool> {
    Some(true)
}

impl From<TaskDefinitionIntermediate> for TaskDefinition {
    fn from(intermediate: TaskDefinitionIntermediate) -> Self {
        Self {
            description: intermediate.description,
            shell: intermediate.shell.map(OsString::from),
            exec: intermediate.exec.map(OsString::from),
            args: intermediate
                .args
                .map(|v| v.into_iter().map(OsString::from).collect()),
            expand_args: intermediate.expand_args,
            sudo: intermediate.sudo,
            hook: intermediate.hook,
            condition: intermediate.condition,
        }
    }
}

impl ConditionEvaluator for ModuleTask {
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

impl ConditionalComponent for ModuleTask {
    fn log_error(&self, module: &str, location: &Path, err: impl std::fmt::Display) {
        error!(
            module,
            location = ?location,
            descrption = if let Some(description) = &self.description {
                description
            } else {
                "No description"
            },
            "Task condition evaluation failed: {}",
            err
        );
    }
}

impl ConditionEvaluator for TaskDefinition {
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

impl ConditionalComponent for TaskDefinition {
    fn log_error(&self, module: &str, location: &Path, err: impl std::fmt::Display) {
        error!(
            module,
            location = ?location,
            command = self.shell.as_deref().map(|c| c.to_string_lossy().to_string()).or(self.exec.as_deref().map(|c| c.to_string_lossy().to_string())).unwrap_or("<invalid>".to_string()),
            "Task condition evaluation failed: {}",
            err
        );
    }
}
