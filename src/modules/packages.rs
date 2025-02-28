use crate::modules::ConditionEvaluator;
use color_eyre::eyre::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ModulePackages {
    /// List of package names to install when conditions are met
    pub(crate) install: Vec<String>,

    /// Optional conditional expression evaluated at runtime
    #[serde(rename = "if")]
    pub(crate) condition: Option<String>,
}

impl ConditionEvaluator for ModulePackages {
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

#[derive(Debug, Default)]
pub(crate) struct InstallPackage {
    pub(crate) module_name: String,
    /// The content of the message to be displayed.
    pub(crate) package: String,
}
