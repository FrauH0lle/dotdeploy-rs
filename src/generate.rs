use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::fs;

use crate::utils::file_fs;
use crate::read_module;

async fn generate_file<P: AsRef<Path>>(
    stores: Arc<(crate::store::db::Store, Option<crate::store::db::Store>)>,
    target: P,
    generator: &read_module::Generate,
    context: &serde_json::Value,
    hb: &handlebars::Handlebars<'static>,
) -> Result<()> {
    let modules = stores
        .0
        .get_all_modules()
        .await
        .map_err(|e| e.into_anyhow())?;
    let mut content = String::new();
    if let Some(prepend) = &generator.prepend {
        let rendered = hb
            .render_template(prepend, &context)
            .with_context(|| format!("Failed to render template {:?}", &prepend))?;

        content.push_str(&rendered);
    }

    for module in modules.iter() {
        let location: PathBuf = [&module.location, &generator.source].iter().collect();
        if location.exists() {
            let found_content = fs::read_to_string(&location).await?;
            let rendered = hb
                .render_template(&found_content, &context)
                .with_context(|| format!("Failed to render template {:?}", &found_content))?;

            content.push_str(&rendered);
        }
    }

    if let Some(append) = &generator.append {
        let rendered = hb
            .render_template(append, &context)
            .with_context(|| format!("Failed to render template {:?}", &append))?;

        content.push_str(&rendered);
    }

    if !content.is_empty() {
        fs::write(&target, content).await?;
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

pub(crate) async fn generate_files(
    stores: Arc<(crate::store::db::Store, Option<crate::store::db::Store>)>,
    generators: std::collections::BTreeMap<std::path::PathBuf, read_module::Generate>,
    context: serde_json::Value,
    hb: Arc<handlebars::Handlebars<'static>>,
) -> Result<()> {
    let mut set = tokio::task::JoinSet::new();
    let context = Arc::new(context);

    // Delete all previously generated files
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
    stores
        .0
        .remove_module("__dotdeploy_generated")
        .await
        .map_err(|e| e.into_anyhow())?;

    for (target, config) in generators.into_iter() {
        let stores_clone = Arc::clone(&stores); // Clone the Arc
        let context_clone = Arc::clone(&context);
        let hb_clone = Arc::clone(&hb);

        set.spawn(
            async move { generate_file(stores_clone, target, &config, &context_clone, &hb_clone).await },
        );
    }

    while let Some(res) = set.join_next().await {
        res??;
    }

    Ok(())
}
