//! Module for handling actions in the dotdeploy system.
//!
//! This module defines structures and functions for managing deployment actions, including their
//! execution methods and conditional logic.

use std::fmt;

use anyhow::{bail, Context, Result};
use serde::de::{self, Error, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

use crate::modules::conditional::Conditional;

/// Represents an individual action within a deployment process.
///
/// This struct defines an action that can be executed during deployment, including the command to
/// run, whether to use sudo, and conditional execution.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub(crate) struct ModuleAction {
    /// The command(s) to be executed by the action.
    pub(crate) exec: RunExec,
    /// Indicates if the command should be run with sudo privileges.
    sudo: bool,
    /// Additional arguments to be passed to the command.
    args: Option<Vec<String>>,
    /// A conditional expression that determines if the action should be executed.
    pub(crate) eval_when: Option<String>,
}

// Custom deserialization implementation for ModuleAction
impl<'de> Deserialize<'de> for ModuleAction {
    // This is where the deserialization process begins.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // A temporary struct to help deserialize the raw data. It mirrors the structure of
        // ModuleAction but includes an additional `exec_file` field to help determine how to
        // interpret the `exec` field.
        #[derive(Deserialize)]
        struct Helper {
            exec: String,              // Raw command or filepath as a string
            exec_file: Option<bool>,   // Indicator if exec is a file
            sudo: Option<bool>,        // Indicator if sudo should be used
            args: Option<Vec<String>>, // Indicator if additional args should be used
            eval_when: Option<String>, // Optional condition for execution
        }

        // Visitor struct for custom processing of the deserialized data.
        struct ModuleActionVisitor;

        // Implementation of Visitor trait for ModuleActionVisitor.
        impl<'de> Visitor<'de> for ModuleActionVisitor {
            type Value = ModuleAction;

            // Describes what this visitor expects to receive.
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct ModuleAction")
            }

            // Custom processing of the map representing deserialized data.
            fn visit_map<V>(self, mut map: V) -> Result<ModuleAction, V::Error>
            where
                V: MapAccess<'de>,
            {
                // Deserialize the map into our helper struct.
                let helper: Helper =
                    Deserialize::deserialize(de::value::MapAccessDeserializer::new(&mut map))?;

                // Decide how to interpret the `exec` field based on `exec_file`. If `exec_file` is
                // true, treat `exec` as a filepath (File variant of Exec). Otherwise, treat `exec`
                // as a command to execute directly (Code variant of Exec).
                let exec = if helper.exec_file.is_some_and(|x| x) {
                    match shellexpand::full(&helper.exec) {
                        Ok(p) => RunExec::File(p.to_string()),
                        Err(e) => {
                            return Err(V::Error::custom(format!("Error expanding path: {}", e)))
                        }
                    }
                } else {
                    RunExec::Code(helper.exec)
                };

                // Construct and return the actual ModuleAction object with our custom logic
                // applied.
                Ok(ModuleAction {
                    exec,
                    eval_when: helper.eval_when,
                    sudo: helper.sudo.unwrap_or(false),
                    args: helper.args,
                })
            }
        }

        // Trigger the custom deserialization process using the visitor pattern. This line
        // effectively starts the deserialization process defined above.
        deserializer.deserialize_map(ModuleActionVisitor)
    }
}

/// Implementation of `Conditional` for `ModuleAction`, providing access to its `eval_when` field.
impl Conditional for ModuleAction {
    fn eval_when(&self) -> &Option<String> {
        &self.eval_when
    }
}

/// Represents the type of execution for an action.
#[derive(Deserialize, Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub(crate) enum RunExec {
    /// Represents a command to be executed directly.
    Code(String),
    /// Represents a file to be executed.
    File(String),
}

impl ModuleAction {
    /// Executes the action.
    ///
    /// This method runs the action based on its configuration, handling both direct code execution
    /// and file execution, with or without sudo.
    pub(crate) async fn run(&self) -> Result<()> {
        match &self.exec {
            RunExec::Code(code) => {
                // Execute the code directly using sh
                let mut cmd = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(code)
                    .spawn()
                    .with_context(|| format!("Failed to run {:?}", &self.exec))?;

                let status = cmd.wait()?;
                if status.success() {
                    Ok(())
                } else {
                    bail!("Failed to execute {:?}", code);
                }
            }
            RunExec::File(file) => {
                let args = self.args.as_deref().unwrap_or(&[]);
                if self.sudo {
                    // Use sudo to execute the file
                    crate::utils::sudo::spawn_sudo_maybe(format!(
                        "Running {:?} with args: {:?}",
                        file, args
                    ))
                    .await
                    .context("Failed to spawn sudo")?;

                    let mut fcmd = vec![file];
                    fcmd.extend(args);

                    let mut cmd = std::process::Command::new("sudo")
                        .args(&fcmd)
                        .spawn()
                        .with_context(|| format!("Failed to run {:?}", fcmd))?;

                    let status = cmd.wait()?;
                    if status.success() {
                        Ok(())
                    } else {
                        bail!("Failed to execute {:?} with args {:?}", file, args);
                    }
                } else {
                    // Execute the file directly
                    let mut cmd = std::process::Command::new(file)
                        .args(args)
                        .spawn()
                        .with_context(|| {
                            format!("Failed to run {:?} with args {:?}", file, args)
                        })?;

                    let status = cmd.wait()?;
                    if status.success() {
                        Ok(())
                    } else {
                        bail!("Failed to execute {:?} with args {:?}", file, args);
                    }
                }
            }
        }
    }
}
