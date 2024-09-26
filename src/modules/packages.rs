//! Module for handling package installations in the dotdeploy configuration.
//!
//! This module defines the structure and behavior of package installations that can be performed
//! during the deployment process. It allows for conditional installation of packages based on
//! custom conditions.

use serde::Deserialize;

use crate::modules::conditional::Conditional;

/// Configuration for package installation within a module.
///
/// This struct represents a set of packages to be installed as part of the deployment process. It
/// includes a list of packages and an optional condition for their installation.
#[derive(Deserialize, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ModulePackages {
    /// A list of package names to install.
    ///
    /// Each string in this vector represents the name of a package that should be installed when
    /// the conditions are met.
    pub(crate) install: Vec<String>,

    /// An optional conditional expression for package installation.
    ///
    /// If provided, this expression is evaluated at runtime. The packages are only installed if the
    /// condition evaluates to true. If not provided, the packages will always be installed (subject
    /// to other deployment rules).
    pub(crate) eval_when: Option<String>,
}

/// Implementation of the `Conditional` trait for `ModulePackages`.
///
/// This implementation allows `ModulePackages` to be used in contexts where conditional evaluation is
/// required, such as when deciding whether to install packages based on runtime conditions.
impl Conditional for ModulePackages {
    fn eval_when(&self) -> &Option<String> {
        // Return a reference to the `eval_when` field, which contains the conditional expression
        // (if any) for this package set
        &self.eval_when
    }
}
