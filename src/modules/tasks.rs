use crate::modules::{DeployPhase, default_phase_hook, default_option_bool, ConditionEvaluator};
use color_eyre::eyre::Result;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Debug, Default)]
pub(crate) struct ModuleTask {
    pub(crate) shell: Option<String>,
    pub(crate) exec: Option<String>,
    pub(crate) args: Option<Vec<String>>,
    #[serde(default = "default_option_bool")]
    pub(crate) sudo: Option<bool>,
    #[serde(default)]
    pub(crate) phase: DeployPhase,
    #[serde(default = "default_phase_hook")]
    pub(crate) hook: Option<String>,
    #[serde(rename = "if")]
    pub(crate) condition: Option<String>,
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
