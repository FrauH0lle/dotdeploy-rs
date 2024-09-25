//! This module handles the generation of files based on templates and module configurations.
//!
//! It provides functionality to generate individual files and manage the generation process for
//! multiple files concurrently.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::fs;

use crate::read_module;
use crate::utils::file_fs;

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
    stores: Arc<(crate::store::db::Store, Option<crate::store::db::Store>)>,
    target: P,
    generator: &read_module::Generate,
    context: &serde_json::Value,
    hb: &handlebars::Handlebars<'static>,
) -> Result<()> {
    // Retrieve all modules from the store
    let modules = stores
        .0
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
            .0
            .add_module(crate::store::modules::StoreModule {
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
            .0
            .add_file(crate::store::files::StoreFile {
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
    stores: Arc<(crate::store::db::Store, Option<crate::store::db::Store>)>,
    generators: std::collections::BTreeMap<std::path::PathBuf, read_module::Generate>,
    context: serde_json::Value,
    hb: Arc<handlebars::Handlebars<'static>>,
) -> Result<()> {
    let mut set = tokio::task::JoinSet::new();
    let context = Arc::new(context);

    // Clean up previously generated files
    let prev_files = stores
        .0
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
        .0
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
