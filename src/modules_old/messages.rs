//! Module for handling messages in the dotdeploy configuration.
//!
//! This module defines the structure and behavior of messages that can be displayed during the
//! deployment or removal process. It allows for conditional display of messages based on the
//! deployment stage and custom conditions.

use serde::Deserialize;

use crate::modules::conditional::Conditional;

/// Configuration for messages within a module.
///
/// This struct represents a message that can be displayed during the deployment or removal process.
/// It includes the message content, when to display it, and an optional condition for its display.
#[derive(Deserialize, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ModuleMessages {
    /// The content of the message to be displayed.
    pub(crate) message: String,

    /// Specifies when to display the message.
    ///
    /// Can be either "deploy" or "remove". If not specified, it defaults to "deploy".
    #[serde(default = "default_message")]
    pub(crate) display_when: String,

    /// An optional conditional expression for displaying the message.
    ///
    /// If provided, this expression is evaluated at runtime. The message is only displayed if the
    /// condition evaluates to true.
    pub(crate) eval_when: Option<String>,
}

/// Provides the default value for the `display_when` field.
///
/// This function is used by Serde to set the default value of `display_when` when it's not
/// specified in the configuration.
fn default_message() -> String {
    // By default, messages are displayed during deployment
    "deploy".to_string()
}

/// Implementation of the `Conditional` trait for `ModuleMessages`.
///
/// This implementation allows `ModuleMessages` to be used in contexts where conditional evaluation is
/// required, such as when deciding whether to display a message based on runtime conditions.
impl Conditional for ModuleMessages {
    fn eval_when(&self) -> &Option<String> {
        // Return a reference to the `eval_when` field, which contains the conditional expression
        // (if any) for this message
        &self.eval_when
    }
}
