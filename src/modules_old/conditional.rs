//! This module provides traits and implementations for conditional evaluation of configuration
//! elements.
//!
//! It allows for flexible and reusable condition checking across different data structures.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use handlebars::Handlebars;
use serde_json::Value;

/// Trait for types that have a condition that can be evaluated.
pub(crate) trait Conditional {
    /// Returns a reference to the condition string, if present.
    fn eval_when(&self) -> &Option<String>;
}

/// Trait defining methods for evaluating conditions on different data structures.
pub(crate) trait ConditionalEvaluator {
    /// Evaluates conditions for a map of PathBuf to Conditional items.
    fn eval_conditional_map<T: Conditional>(
        &self,
        map: Option<BTreeMap<PathBuf, T>>,
        context: &Value,
        hb: &Handlebars<'static>,
    ) -> Result<Option<BTreeMap<PathBuf, T>>>;

    /// Evaluates conditions for a nested map structure.
    fn eval_conditional_nested_map<T: Conditional>(
        &self,
        map: Option<BTreeMap<String, BTreeMap<String, Vec<T>>>>,
        context: &Value,
        hb: &Handlebars<'static>,
    ) -> Result<Option<BTreeMap<String, BTreeMap<String, Vec<T>>>>>;

    /// Evaluates conditions for a vector of Conditional items.
    fn eval_conditional_vec<T: Conditional>(
        &self,
        vec: Option<Vec<T>>,
        context: &Value,
        hb: &Handlebars<'static>,
    ) -> Result<Option<Vec<T>>>;

    /// Wrapper method to evaluate the condition of a single Conditional item.
    fn eval_condition_wrapper<T: Conditional>(
        &self,
        item: &T,
        context: &Value,
        hb: &Handlebars<'static>,
    ) -> bool;
}

/// Default implementation of the ConditionalEvaluator trait.
pub(crate) struct DefaultConditionalEvaluator;

impl ConditionalEvaluator for DefaultConditionalEvaluator {
    fn eval_conditional_map<T: Conditional>(
        &self,
        mut map: Option<BTreeMap<PathBuf, T>>,
        context: &Value,
        hb: &Handlebars<'static>,
    ) -> Result<Option<BTreeMap<PathBuf, T>>> {
        // If the map exists, evaluate conditions for each item
        map.as_mut().map(|m| {
            m.retain(|_, value| self.eval_condition_wrapper(value, context, hb));
        });
        // Return None if the map is empty after evaluation
        Ok(map.filter(|m| !m.is_empty()))
    }

    fn eval_conditional_nested_map<T: Conditional>(
        &self,
        mut map: Option<BTreeMap<String, BTreeMap<String, Vec<T>>>>,
        context: &Value,
        hb: &Handlebars<'static>,
    ) -> Result<Option<BTreeMap<String, BTreeMap<String, Vec<T>>>>> {
        // If the nested map exists, evaluate conditions for each item
        map.as_mut().map(|outer_map| {
            outer_map.retain(|_, inner_map| {
                inner_map.retain(|_, actions| {
                    // Evaluate conditions for each action in the innermost Vec
                    actions.retain(|action| self.eval_condition_wrapper(action, context, hb));
                    // Remove empty Vec
                    !actions.is_empty()
                });
                // Remove empty inner maps
                !inner_map.is_empty()
            });
        });
        // Return None if the outer map is empty after evaluation
        Ok(map.filter(|m| !m.is_empty()))
    }

    fn eval_conditional_vec<T: Conditional>(
        &self,
        mut vec: Option<Vec<T>>,
        context: &Value,
        hb: &Handlebars<'static>,
    ) -> Result<Option<Vec<T>>> {
        // If the vector exists, evaluate conditions for each item
        vec.as_mut().map(|v| {
            v.retain(|item| self.eval_condition_wrapper(item, context, hb));
        });
        // Return None if the vector is empty after evaluation
        Ok(vec.filter(|v| !v.is_empty()))
    }

    fn eval_condition_wrapper<T: Conditional>(
        &self,
        item: &T,
        context: &Value,
        hb: &Handlebars<'static>,
    ) -> bool {
        match item.eval_when() {
            Some(cond) => match eval_condition(cond, context, hb) {
                Ok(true) => true,
                Ok(false) => false,
                Err(e) => {
                    // Log the error and treat it as a failed condition
                    error!("Error during condition evaluation:\n {}", e);
                    false
                }
            },
            // If there's no condition, the item is always included
            None => true,
        }
    }
}

/// Evaluate the handlebars template in the `eval_when` field.
///
/// This function constructs a Handlebars template that will return "true" or "false" based on the
/// evaluation of the condition. It then renders this template with the provided context and
/// interprets the result.
fn eval_condition(condition: &str, context: &Value, hb: &Handlebars<'static>) -> Result<bool> {
    // Construct a Handlebars template that will evaluate to "true" or "false"
    let eval_template = format!(
        "{{{{#if {condition}}}}}true{{{{else}}}}false{{{{/if}}}}",
        condition = condition
    );

    // Render the template with the provided context
    let result = hb
        .render_template(&eval_template, context)
        .with_context(|| format!("Failed to evaluate template: {}", eval_template))?;

    // Interpret the result
    match result.as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        // Any other result is treated as false
        _ => Ok(false),
    }
}

//
// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::files::ModuleFile;

    #[test]
    fn test_eval_condition() {
        let hb = Handlebars::new();
        let context = serde_json::json!({
            "foo": true,
            "bar": false,
            "num": 42
        });

        assert!(eval_condition("foo", &context, &hb).unwrap());
        assert!(!eval_condition("bar", &context, &hb).unwrap());
        assert!(eval_condition("(gt num 40)", &context, &hb).unwrap());
        assert!(!eval_condition("(lt num 40)", &context, &hb).unwrap());
        assert!(eval_condition("(and foo (gt num 40))", &context, &hb).unwrap());
    }

    #[test]
    fn test_eval_conditional_map() {
        let evaluator = DefaultConditionalEvaluator;
        let hb = Handlebars::new();
        let context = serde_json::json!({ "include": true });

        let mut map = BTreeMap::new();
        map.insert(
            PathBuf::from("/test1"),
            ModuleFile {
                eval_when: Some("include".to_string()),
                ..Default::default()
            },
        );
        map.insert(
            PathBuf::from("/test2"),
            ModuleFile {
                eval_when: Some("not_include".to_string()),
                ..Default::default()
            },
        );
        map.insert(
            PathBuf::from("/test3"),
            ModuleFile {
                eval_when: None,
                ..Default::default()
            },
        );

        let result = evaluator
            .eval_conditional_map(Some(map), &context, &hb)
            .unwrap();
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.contains_key(&PathBuf::from("/test1")));
        assert!(result.contains_key(&PathBuf::from("/test3")));
    }

    #[test]
    fn test_eval_conditional_nested_map() {
        let evaluator = DefaultConditionalEvaluator;
        let hb = Handlebars::new();
        let context = serde_json::json!({ "include": true });

        let mut nested_map = BTreeMap::new();
        let mut inner_map = BTreeMap::new();
        inner_map.insert(
            "stage".to_string(),
            vec![
                ModuleFile {
                    eval_when: Some("include".to_string()),
                    ..Default::default()
                },
                ModuleFile {
                    eval_when: Some("not_include".to_string()),
                    ..Default::default()
                },
            ],
        );
        nested_map.insert("phase".to_string(), inner_map);

        let result = evaluator
            .eval_conditional_nested_map(Some(nested_map), &context, &hb)
            .unwrap();
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result["phase"]["stage"].len(), 1);
    }

    #[test]
    fn test_eval_conditional_vec() {
        let evaluator = DefaultConditionalEvaluator;
        let hb = Handlebars::new();
        let context = serde_json::json!({ "include": true });

        let vec = vec![
            ModuleFile {
                eval_when: Some("include".to_string()),
                ..Default::default()
            },
            ModuleFile {
                eval_when: Some("not_include".to_string()),
                ..Default::default()
            },
            ModuleFile {
                eval_when: None,
                ..Default::default()
            },
        ];

        let result = evaluator
            .eval_conditional_vec(Some(vec), &context, &hb)
            .unwrap();
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_eval_condition_wrapper() {
        let evaluator = DefaultConditionalEvaluator;
        let hb = Handlebars::new();
        let context = serde_json::json!({ "include": true });

        let item1 = ModuleFile {
            eval_when: Some("include".to_string()),
            ..Default::default()
        };
        let item2 = ModuleFile {
            eval_when: Some("not_include".to_string()),
            ..Default::default()
        };
        let item3 = ModuleFile {
            eval_when: None,
            ..Default::default()
        };

        assert!(evaluator.eval_condition_wrapper(&item1, &context, &hb));
        assert!(!evaluator.eval_condition_wrapper(&item2, &context, &hb));
        assert!(evaluator.eval_condition_wrapper(&item3, &context, &hb));
    }
}
