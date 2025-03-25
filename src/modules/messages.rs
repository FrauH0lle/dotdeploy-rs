use crate::modules::{default_on_command, ConditionEvaluator, ConditionalComponent};
use std::path::Path;
use tracing::error;
use color_eyre::eyre::Result;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct ModuleMessage {
    /// The content of the message to be displayed.
    pub(crate) message: String,
    /// Specifies when to display the message.
    ///
    /// Can be either "deploy", "remove" or "update". If not specified, it defaults to "deploy".
    #[serde(default = "default_on_command")]
    pub(crate) on_command: Option<String>,
    /// An optional conditional expression for displaying the message.
    ///
    /// If provided, this expression is evaluated at runtime. The message is only displayed if the
    /// condition evaluates to true.
    #[serde(rename = "if")]
    pub(crate) condition: Option<String>,
}

impl ConditionEvaluator for ModuleMessage {
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

impl ConditionalComponent for ModuleMessage {
    fn log_error(&self, module: &str, location: &Path, err: impl std::fmt::Display) {
        error!(
            module,
            location = ?location,
            message = self.message,
            "Message condition evaluation failed: {}",
            err
        );
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommandMessage {
    pub(crate) module_name: String,
    /// The content of the message to be displayed.
    pub(crate) message: String,
    /// Specifies when to display the message.
    ///
    /// Can be either "deploy", "remove" or "update". If not specified, it defaults to "deploy".
    pub(crate) on_command: Option<String>,
}
