//! Module for handling file generation in the dotdeploy configuration.
//!
//! This module defines the structure and behavior of file generation that can be performed during
//! the deployment process. It provides functionality to generate individual files and manage the
//! generation process for multiple files concurrently.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use handlebars::Handlebars;
use serde::Deserialize;
use serde_json::Value;
use tokio::fs;

use crate::modules::conditional::Conditional;
use crate::store::Stores;
use crate::store::db::Store;
use crate::store::files::StoreFile;
use crate::store::modules::StoreModule;
use crate::utils::file_fs;

/// Configuration for file generation within a module.
///
/// This struct represents the configuration for generating a file as part of the deployment
/// process. It includes options for prepending and appending content, specifying a source file, and
/// conditional generation.
#[derive(Deserialize, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Generate {
    /// Optional content to prepend to the generated file.
    ///
    /// If provided, this content will be added at the beginning of the generated file.
    pub(crate) prepend: Option<String>,

    /// The name or path of the source file.
    ///
    /// This specifies the main content source for the generated file. It could be a template file
    /// or a regular file that will be processed.
    pub(crate) source: String,

    /// Optional content to append to the generated file.
    ///
    /// If provided, this content will be added at the end of the generated file.
    pub(crate) append: Option<String>,

    /// An optional conditional expression for file generation.
    ///
    /// If provided, this expression is evaluated at runtime. The file is only generated if the
    /// condition evaluates to true. If not provided, the file will always be generated (subject to
    /// other deployment rules).
    pub(crate) eval_when: Option<String>,
}

/// Implementation of the `Conditional` trait for `Generate`.
///
/// This implementation allows `Generate` to be used in contexts where conditional evaluation is
/// required, such as when deciding whether to generate a file based on runtime conditions.
impl Conditional for Generate {
    fn eval_when(&self) -> &Option<String> {
        // Return a reference to the `eval_when` field, which contains the conditional expression
        // (if any) for this file generation
        &self.eval_when
    }
}

/// Generates a single file based on the provided configuration and context.
///
/// This function collects content from multiple modules, applies templates, and writes the result
/// to the target file.
///
/// # Arguments
///
/// * `stores` - Arc-wrapped tuple of database stores (user and optional system store)
/// * `target` - The path where the generated file will be written
/// * `generator` - Configuration for the file generation
/// * `context` - JSON context for template rendering
/// * `hb` - Handlebars instance for template rendering
///
/// # Returns
///
/// A Result indicating success or failure of the file generation process
async fn generate_file<P: AsRef<Path>>(
    stores: Arc<Stores>,
    target: P,
    generator: &Generate,
    context: &Value,
    hb: &Handlebars<'static>,
) -> Result<()> {
    // Retrieve all modules from the store
    let modules = stores
        .user_store
        .get_all_modules()
        .await
        .map_err(|e| e.into_anyhow())?;

    let mut content = String::new();

    // Handle prepend content if present
    if let Some(prepend) = &generator.prepend {
        let rendered = hb
            .render_template(prepend, &context)
            .with_context(|| format!("Failed to render template {:?}", &prepend))?;

        content.push_str(&rendered);
    }

    // Iterate through all modules and collect relevant content
    for module in modules.iter() {
        let location: PathBuf = [&module.location, &generator.source].iter().collect();
        if location.exists() {
            // Read and render the content from each module
            let found_content = fs::read_to_string(&location).await?;
            let rendered = hb
                .render_template(&found_content, &context)
                .with_context(|| format!("Failed to render template {:?}", &found_content))?;

            content.push_str(&rendered);
        }
    }

    // Handle append content if present
    if let Some(append) = &generator.append {
        let rendered = hb
            .render_template(append, &context)
            .with_context(|| format!("Failed to render template {:?}", &append))?;

        content.push_str(&rendered);
    }

    // Write the generated content to the target file if not empty
    if !content.is_empty() {
        fs::write(&target, content).await?;

        // Add a special module entry for generated content
        stores
            .user_store
            .add_module(StoreModule {
                name: "__dotdeploy_generated".to_string(),
                location: std::env::var("DOD_MODULES_ROOT")?,
                user: Some(std::env::var("USER")?),
                reason: "automatic".to_string(),
                depends: None,
                date: chrono::offset::Local::now(),
            })
            .await
            .map_err(|e| e.into_anyhow())?;

        // Add the generated file to the store
        stores
            .user_store
            .add_file(StoreFile {
                module: "__dotdeploy_generated".to_string(),
                source: None,
                source_checksum: None,
                destination: file_fs::path_to_string(target)?,
                destination_checksum: None,
                operation: "generate".to_string(),
                user: Some(std::env::var("USER")?),
                date: chrono::offset::Local::now(),
            })
            .await
            .map_err(|e| e.into_anyhow())?;
    }

    Ok(())
}

/// Generates multiple files concurrently based on the provided configurations.
///
/// This function manages the generation of multiple files, handling cleanup of previously generated
/// files and coordinating concurrent generation tasks.
///
/// # Arguments
///
/// * `stores` - Arc-wrapped tuple of database stores (user and optional system store)
/// * `generators` - Map of target paths to their respective generation configurations
/// * `context` - JSON context for template rendering
/// * `hb` - Arc-wrapped Handlebars instance for template rendering
///
/// # Returns
///
/// A Result indicating success or failure of the overall file generation process
pub(crate) async fn generate_files(
    stores: Arc<Stores>,
    generators: BTreeMap<PathBuf, Generate>,
    context: Value,
    hb: Arc<Handlebars<'static>>,
) -> Result<()> {
    let mut set = tokio::task::JoinSet::new();
    let context = Arc::new(context);

    // Clean up previously generated files
    let prev_files = stores
        .user_store
        .get_all_files("__dotdeploy_generated")
        .await
        .map_err(|e| e.into_anyhow())?;
    if !prev_files.is_empty() {
        for f in prev_files.into_iter() {
            file_fs::delete_file(f.destination).await?;
        }
    }

    // Remove the special generated module from the store
    stores
        .user_store
        .remove_module("__dotdeploy_generated")
        .await
        .map_err(|e| e.into_anyhow())?;

    // Spawn concurrent tasks for each file generation
    for (target, config) in generators.into_iter() {
        let stores_clone = Arc::clone(&stores);
        let context_clone = Arc::clone(&context);
        let hb_clone = Arc::clone(&hb);

        set.spawn(async move {
            generate_file(stores_clone, target, &config, &context_clone, &hb_clone).await
        });
    }

    // Wait for all generation tasks to complete
    while let Some(res) = set.join_next().await {
        res??;
    }

    Ok(())
}
