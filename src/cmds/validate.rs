use crate::config::DotdeployConfig;
use crate::store::Store;
use crate::store::sqlite::SQLiteStore;
use crate::store::sqlite_files::StoreFile;
use crate::utils::FileUtils;
use crate::utils::common::os_str_to_bytes;
use crate::utils::sudo::PrivilegeManager;
use color_eyre::Result;
use color_eyre::eyre::{WrapErr, eyre};
use handlebars::Handlebars;
use similar::{ChangeTag, TextDiff};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::IsTerminal;
use std::path::Path;
use std::sync::Arc;
use tokio::fs;
use toml::Value;
use tracing::{debug, info, warn};

/// Represents a file that has diverged between the store and the filesystem.
struct DivergedFile {
    store_file: StoreFile,
    stored_checksum: String,
    current_checksum: String,
}

/// Validation context for diverged file resolution.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValidationMode {
    Standalone,
    SyncPreflight,
}

/// User resolution choice for a diverged file.
#[derive(Clone, Copy)]
enum Resolution {
    KeepSource,
    KeepDeployed,
    Merge,
    Skip,
}

/// Validates all deployed non-symlink files and interactively resolves divergences.
pub(crate) async fn validate(
    _config: Arc<DotdeployConfig>,
    store: Arc<SQLiteStore>,
    context: HashMap<String, Value>,
    handlebars: Handlebars<'static>,
    pm: Arc<PrivilegeManager>,
) -> Result<bool> {
    validate_deployed_files_interactively(
        ValidationMode::Standalone,
        Arc::clone(&store),
        &context,
        &handlebars,
        pm,
        None,
        false,
    )
    .await
}

/// Validates deployed files and interactively resolves divergences.
pub(crate) async fn validate_deployed_files_interactively(
    mode: ValidationMode,
    store: Arc<SQLiteStore>,
    context: &HashMap<String, Value>,
    handlebars: &Handlebars<'static>,
    pm: Arc<PrivilegeManager>,
    target_scope: Option<&HashSet<String>>,
    force: bool,
) -> Result<bool> {
    let file_utils = FileUtils::new(pm);
    let diverged = collect_diverged_files(&store, &file_utils, target_scope).await?;

    if diverged.is_empty() {
        info!("All deployed files are in sync with the store");
        return Ok(true);
    }

    info!(
        "Found {} diverged file{}",
        diverged.len(),
        if diverged.len() == 1 { "" } else { "s" }
    );

    if force && mode == ValidationMode::SyncPreflight {
        warn!(
            "The following files were modified outside of dotdeploy and will be overwritten by sync due to --force:{}",
            format!(
                "\n  - {}",
                diverged
                    .iter()
                    .map(|df| df.store_file.target.as_str())
                    .collect::<Vec<_>>()
                    .join("\n  - ")
            )
        );
        return Ok(true);
    }

    let Some(resolutions) = review_diverged_files(&diverged, &file_utils, mode).await? else {
        info!("Validate aborted before applying resolutions");
        return Ok(false);
    };

    apply_resolutions(
        &diverged,
        &resolutions,
        &file_utils,
        &store,
        context,
        handlebars,
        mode,
    )
    .await?;

    Ok(true)
}

async fn collect_diverged_files(
    store: &SQLiteStore,
    file_utils: &FileUtils,
    target_scope: Option<&HashSet<String>>,
) -> Result<Vec<DivergedFile>> {
    let modules = store.get_all_modules().await?;
    let mut diverged = Vec::new();

    for module in &modules {
        let files = store.get_all_files(&module.name).await?;
        for file in files {
            // Skip symlinks — they don't have checksums to validate
            if file.operation == "link" {
                continue;
            }

            if let Some(target_scope) = target_scope
                && !target_scope.contains(&file.target)
            {
                continue;
            }

            // Check if the target still exists
            if !file_utils.check_path_exists(&file.target).await? {
                warn!("{}: target no longer exists on disk", &file.target);
                continue;
            }

            // Compare current target checksum to stored target checksum
            let stored_checksum = match &file.target_checksum {
                Some(c) => c.clone(),
                None => {
                    debug!("{}: no stored target checksum, skipping", &file.target);
                    continue;
                }
            };

            let current_checksum = file_utils
                .calculate_sha256_checksum(&file.target)
                .await
                .wrap_err_with(|| format!("Failed to calculate checksum of {}", &file.target))?;

            if current_checksum != stored_checksum {
                diverged.push(DivergedFile {
                    store_file: file,
                    stored_checksum,
                    current_checksum,
                });
            }
        }
    }

    Ok(diverged)
}

async fn review_diverged_files(
    diverged: &[DivergedFile],
    file_utils: &FileUtils,
    mode: ValidationMode,
) -> Result<Option<Vec<Resolution>>> {
    let mut resolutions = vec![None; diverged.len()];
    let mut index = 0;

    loop {
        let df = &diverged[index];
        let is_binary = show_diverged_file(
            df,
            index,
            diverged.len(),
            resolutions[index],
            file_utils,
            mode,
        )
        .await?;

        match prompt_review_action(&df.store_file, is_binary, diverged.len(), mode) {
            ReviewAction::Resolve(resolution) => {
                resolutions[index] = Some(resolution);
                if resolutions.iter().all(Option::is_some) {
                    match prompt_apply_summary(diverged, &resolutions, mode) {
                        SummaryAction::Apply => {
                            return Ok(Some(resolutions.into_iter().flatten().collect()));
                        }
                        SummaryAction::ReviewAgain => {
                            index = 0;
                        }
                        SummaryAction::Quit => return Ok(None),
                    }
                } else {
                    index = next_review_index(index, diverged.len());
                }
            }
            ReviewAction::Next => {
                index = next_review_index(index, diverged.len());
            }
            ReviewAction::Previous => {
                index = previous_review_index(index, diverged.len());
            }
            ReviewAction::Quit => return Ok(None),
        }
    }
}

async fn show_diverged_file(
    df: &DivergedFile,
    index: usize,
    total: usize,
    resolution: Option<Resolution>,
    file_utils: &FileUtils,
    mode: ValidationMode,
) -> Result<bool> {
    let file = &df.store_file;
    let is_binary = if let Ok(bytes) = std::fs::read(&file.target) {
        is_binary_content(&bytes)
    } else {
        true
    };

    eprintln!("\n--- {} --- [{}/{}]", &file.target, index + 1, total);
    eprintln!("  module:    {}", &file.module);
    eprintln!("  operation: {}", &file.operation);
    eprintln!("  resolution: {}", resolution_label(resolution, mode));
    eprintln!("  stored checksum:  {}", &df.stored_checksum);
    eprintln!("  current checksum: {}", &df.current_checksum);

    if is_binary {
        eprintln!("  (binary file — diff not shown)");
    } else if file.operation == "copy" {
        // For copied files, diff source vs deployed target
        if let Some(ref source) = file.source {
            if file_utils.check_path_exists(source).await? {
                let source_content = read_file_content(source, file_utils).await?;
                let target_content = read_file_content(&file.target, file_utils).await?;
                show_diff(&source_content, &target_content, source, &file.target);
            } else {
                eprintln!(
                    "  (source file {} no longer exists — diff not shown)",
                    source
                );
            }
        }
    } else if file.operation == "create" || file.operation == "generate" {
        // For created/generated files, we can't diff against source (there is none)
        eprintln!("  (created/generated file — no source to diff against)");
    }

    Ok(is_binary)
}

fn prompt_review_action(
    file: &StoreFile,
    is_binary: bool,
    total: usize,
    mode: ValidationMode,
) -> ReviewAction {
    let has_source = file.source.is_some() && file.operation == "copy";
    let navigation = if total > 1 {
        " | [n]ext | [p]revious"
    } else {
        ""
    };
    let source_action = match mode {
        ValidationMode::Standalone => "overwrite deployed",
        ValidationMode::SyncPreflight => "overwrite deployed now",
    };
    let skip_action = match mode {
        ValidationMode::Standalone => "",
        ValidationMode::SyncPreflight => " (overwrite during sync)",
    };

    eprintln!();

    if is_binary {
        if has_source {
            let prompt = format!(
                "  [s]ource ({}) | [d]eployed (overwrite source) | s[k]ip{}{} | [q]uit?",
                source_action, skip_action, navigation
            );
            match crate::utils::common::ask_choice(&prompt, &review_options(false, total)) {
                's' => ReviewAction::Resolve(Resolution::KeepSource),
                'd' => ReviewAction::Resolve(Resolution::KeepDeployed),
                'n' => ReviewAction::Next,
                'p' => ReviewAction::Previous,
                'q' => ReviewAction::Quit,
                _ => ReviewAction::Resolve(Resolution::Skip),
            }
        } else {
            let prompt = format!("  s[k]ip{}{} | [q]uit?", skip_action, navigation);
            match crate::utils::common::ask_choice(&prompt, &skip_review_options(total)) {
                'n' => ReviewAction::Next,
                'p' => ReviewAction::Previous,
                'q' => ReviewAction::Quit,
                _ => ReviewAction::Resolve(Resolution::Skip),
            }
        }
    } else if has_source {
        let prompt = format!(
            "  [s]ource ({}) | [d]eployed (overwrite source) | [m]erge | s[k]ip{}{} | [q]uit?",
            source_action, skip_action, navigation
        );
        match crate::utils::common::ask_choice(&prompt, &review_options(true, total)) {
            's' => ReviewAction::Resolve(Resolution::KeepSource),
            'd' => ReviewAction::Resolve(Resolution::KeepDeployed),
            'm' => ReviewAction::Resolve(Resolution::Merge),
            'n' => ReviewAction::Next,
            'p' => ReviewAction::Previous,
            'q' => ReviewAction::Quit,
            _ => ReviewAction::Resolve(Resolution::Skip),
        }
    } else {
        // create/generate — can only skip
        let prompt = format!("  s[k]ip{}{} | [q]uit?", skip_action, navigation);
        match crate::utils::common::ask_choice(&prompt, &skip_review_options(total)) {
            'n' => ReviewAction::Next,
            'p' => ReviewAction::Previous,
            'q' => ReviewAction::Quit,
            _ => ReviewAction::Resolve(Resolution::Skip),
        }
    }
}

fn review_options(include_merge: bool, total: usize) -> Vec<char> {
    let mut options = vec!['s', 'd', 'k', 'q'];
    if include_merge {
        options.push('m');
    }
    if total > 1 {
        options.extend(['n', 'p']);
    }
    options
}

fn skip_review_options(total: usize) -> Vec<char> {
    let mut options = vec!['k', 'q'];
    if total > 1 {
        options.extend(['n', 'p']);
    }
    options
}

fn prompt_apply_summary(
    diverged: &[DivergedFile],
    resolutions: &[Option<Resolution>],
    mode: ValidationMode,
) -> SummaryAction {
    eprintln!("\nResolution summary:");
    if mode == ValidationMode::SyncPreflight {
        eprintln!("Skipped files will be overwritten by sync.");
    }
    for (df, resolution) in diverged.iter().zip(resolutions.iter()) {
        eprintln!(
            "  {}  {}",
            &df.store_file.target,
            resolution_label(*resolution, mode)
        );
    }

    let prompt = match mode {
        ValidationMode::Standalone => "\n  [a]pply | [r]eview again | [q]uit without changes?",
        ValidationMode::SyncPreflight => {
            "\n  [a]pply resolutions and continue sync | [r]eview again | [q]uit without changes?"
        }
    };

    match crate::utils::common::ask_choice(prompt, &['a', 'r', 'q']) {
        'a' => SummaryAction::Apply,
        'r' => SummaryAction::ReviewAgain,
        _ => SummaryAction::Quit,
    }
}

async fn apply_resolutions(
    diverged: &[DivergedFile],
    resolutions: &[Resolution],
    file_utils: &FileUtils,
    store: &SQLiteStore,
    context: &HashMap<String, Value>,
    handlebars: &Handlebars<'static>,
    mode: ValidationMode,
) -> Result<()> {
    for (df, resolution) in diverged.iter().zip(resolutions.iter()) {
        let file = &df.store_file;
        match resolution {
            Resolution::KeepSource => {
                resolve_keep_source(file, file_utils, store, context, handlebars).await?;
                info!("{}: redeployed from source", &file.target);
            }
            Resolution::KeepDeployed => {
                resolve_keep_deployed(file, file_utils, store).await?;
                info!("{}: updated source from deployed", &file.target);
            }
            Resolution::Merge => {
                resolve_merge(file, file_utils, store).await?;
                info!("{}: merged", &file.target);
            }
            Resolution::Skip => match mode {
                ValidationMode::Standalone => {
                    info!("{}: skipped", &file.target);
                }
                ValidationMode::SyncPreflight => {
                    info!(
                        "{}: skipped; sync will overwrite deployed target",
                        &file.target
                    );
                }
            },
        }
    }

    Ok(())
}

fn next_review_index(index: usize, total: usize) -> usize {
    (index + 1) % total
}

fn previous_review_index(index: usize, total: usize) -> usize {
    if index == 0 { total - 1 } else { index - 1 }
}

fn resolution_label(resolution: Option<Resolution>, mode: ValidationMode) -> &'static str {
    match (resolution, mode) {
        (Some(Resolution::KeepSource), _) => "source -> deployed",
        (Some(Resolution::KeepDeployed), _) => "deployed -> source",
        (Some(Resolution::Merge), _) => "merge",
        (Some(Resolution::Skip), ValidationMode::Standalone) => "skip",
        (Some(Resolution::Skip), ValidationMode::SyncPreflight) => {
            "skip — sync will overwrite deployed"
        }
        (None, _) => "pending",
    }
}

enum ReviewAction {
    Resolve(Resolution),
    Next,
    Previous,
    Quit,
}

enum SummaryAction {
    Apply,
    ReviewAgain,
    Quit,
}

/// Display a diff between two strings.
fn show_diff(source_content: &str, target_content: &str, source_label: &str, target_label: &str) {
    let old_file = format!("source: {}", source_label);
    let new_file = format!("deployed: {}", target_label);

    if source_content == target_content {
        eprintln!(
            "  (files are identical in content but checksums differ — possible metadata change)"
        );
    } else if should_color_stderr() {
        show_colored_inline_diff(source_content, target_content, &old_file, &new_file);
    } else {
        let diff = TextDiff::from_lines(source_content, target_content);
        let unified = diff.unified_diff().header(&old_file, &new_file).to_string();
        eprintln!("{}", unified);
    }
}

/// Display a colored inline diff, following similar's terminal-inline example style.
fn show_colored_inline_diff(
    source_content: &str,
    target_content: &str,
    old_file: &str,
    new_file: &str,
) {
    let old = source_content.to_string();
    let new = target_content.to_string();
    let diff = TextDiff::from_lines(&old, &new);

    eprintln!("{}", ansi("2", format!("--- {}", old_file)));
    eprintln!("{}", ansi("2", format!("+++ {}", new_file)));

    for (idx, group) in diff.grouped_ops(3).iter().enumerate() {
        if idx > 0 {
            eprintln!("{}", ansi("2", format!("{:-^1$}", "-", 80)));
        }

        for op in group {
            for change in diff.iter_inline_changes(op) {
                let (sign, sign_style, normal_style, emphasized_style) = match change.tag() {
                    ChangeTag::Delete => ("-", "1;31", "31", "31;4;40"),
                    ChangeTag::Insert => ("+", "1;32", "32", "32;4;40"),
                    ChangeTag::Equal => (" ", "1;2", "2", "2"),
                };

                eprint!(
                    "{}{} |{}",
                    ansi("2", Line(change.old_index())),
                    ansi("2", Line(change.new_index())),
                    ansi(sign_style, sign),
                );

                for (emphasized, value) in change.iter_strings_lossy() {
                    if emphasized {
                        eprint!("{}", ansi(emphasized_style, value));
                    } else {
                        eprint!("{}", ansi(normal_style, value));
                    }
                }

                if change.missing_newline() {
                    eprintln!();
                }
            }
        }
    }
}

fn should_color_stderr() -> bool {
    std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

fn ansi(code: &str, value: impl fmt::Display) -> String {
    format!("\x1b[{code}m{value}\x1b[0m")
}

struct Line(Option<usize>);

impl fmt::Display for Line {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            None => write!(f, "    "),
            Some(idx) => write!(f, "{:<4}", idx + 1),
        }
    }
}

/// Redeploy the source file to the target, updating the store checksum.
async fn resolve_keep_source(
    file: &StoreFile,
    file_utils: &FileUtils,
    store: &SQLiteStore,
    _context: &HashMap<String, Value>,
    _handlebars: &Handlebars<'static>,
) -> Result<()> {
    let source = file
        .source
        .as_ref()
        .ok_or_else(|| eyre!("No source path for {}", &file.target))?;

    file_utils
        .copy_file(Path::new(source), Path::new(&file.target))
        .await
        .wrap_err_with(|| format!("Failed to copy {} -> {}", source, &file.target))?;

    // Update store with new checksums
    let new_target_checksum = file_utils.calculate_sha256_checksum(&file.target).await?;
    let new_source_checksum = file_utils.calculate_sha256_checksum(source).await?;

    let updated = crate::store::sqlite_files::StoreFileBuilder::default()
        .with_module(&file.module)
        .with_source(file.source.clone())
        .with_source_u8(file.source_u8.clone())
        .with_source_checksum(Some(new_source_checksum))
        .with_target(&file.target)
        .with_target_u8(os_str_to_bytes(&file.target))
        .with_target_checksum(Some(new_target_checksum))
        .with_operation(&file.operation)
        .with_user(Some(whoami::username()))
        .with_date(chrono::offset::Utc::now())
        .build()?;

    store.add_file(updated).await?;
    Ok(())
}

/// Copy the deployed target back to the source, updating the store.
async fn resolve_keep_deployed(
    file: &StoreFile,
    file_utils: &FileUtils,
    store: &SQLiteStore,
) -> Result<()> {
    let source = file
        .source
        .as_ref()
        .ok_or_else(|| eyre!("No source path for {}", &file.target))?;

    // Copy target -> source
    file_utils
        .copy_file(Path::new(&file.target), Path::new(source))
        .await
        .wrap_err_with(|| format!("Failed to copy {} -> {}", &file.target, source))?;

    // Update store with new checksums
    let new_target_checksum = file_utils.calculate_sha256_checksum(&file.target).await?;
    let new_source_checksum = file_utils.calculate_sha256_checksum(source).await?;

    let updated = crate::store::sqlite_files::StoreFileBuilder::default()
        .with_module(&file.module)
        .with_source(file.source.clone())
        .with_source_u8(file.source_u8.clone())
        .with_source_checksum(Some(new_source_checksum))
        .with_target(&file.target)
        .with_target_u8(os_str_to_bytes(&file.target))
        .with_target_checksum(Some(new_target_checksum))
        .with_operation(&file.operation)
        .with_user(Some(whoami::username()))
        .with_date(chrono::offset::Utc::now())
        .build()?;

    store.add_file(updated).await?;
    Ok(())
}

/// Open a merge tool on the source and target, then update the store.
async fn resolve_merge(
    file: &StoreFile,
    file_utils: &FileUtils,
    store: &SQLiteStore,
) -> Result<()> {
    let source = file
        .source
        .as_ref()
        .ok_or_else(|| eyre!("No source path for {}", &file.target))?;

    // Try $MERGE_TOOL, then $EDITOR, then fall back to "vimdiff"
    let tool = std::env::var("MERGE_TOOL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vimdiff".to_string());

    let status = tokio::process::Command::new(&tool)
        .arg(source)
        .arg(&file.target)
        .status()
        .await
        .wrap_err_with(|| format!("Failed to launch merge tool '{}'", &tool))?;

    if !status.success() {
        warn!(
            "Merge tool '{}' exited with status {}",
            &tool,
            status.code().unwrap_or(-1)
        );
    }

    // After merge, redeploy source to target and update checksums
    file_utils
        .copy_file(Path::new(source), Path::new(&file.target))
        .await
        .wrap_err_with(|| format!("Failed to copy {} -> {}", source, &file.target))?;

    let new_target_checksum = file_utils.calculate_sha256_checksum(&file.target).await?;
    let new_source_checksum = file_utils.calculate_sha256_checksum(source).await?;

    let updated = crate::store::sqlite_files::StoreFileBuilder::default()
        .with_module(&file.module)
        .with_source(file.source.clone())
        .with_source_u8(file.source_u8.clone())
        .with_source_checksum(Some(new_source_checksum))
        .with_target(&file.target)
        .with_target_u8(os_str_to_bytes(&file.target))
        .with_target_checksum(Some(new_target_checksum))
        .with_operation(&file.operation)
        .with_user(Some(whoami::username()))
        .with_date(chrono::offset::Utc::now())
        .build()?;

    store.add_file(updated).await?;
    Ok(())
}

/// Check if content appears to be binary by looking for null bytes in the first 8192 bytes.
fn is_binary_content(bytes: &[u8]) -> bool {
    let check_len = bytes.len().min(8192);
    bytes[..check_len].contains(&0)
}

/// Read file content as a string, with privilege elevation fallback.
async fn read_file_content(path: &str, file_utils: &FileUtils) -> Result<String> {
    match fs::read_to_string(path).await {
        Ok(content) => Ok(content),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            let output = file_utils
                .privilege_manager()
                .sudo_exec_output("cat", [path], None)
                .await?;
            String::from_utf8(output.stdout)
                .wrap_err_with(|| format!("Failed to read {} as UTF-8", path))
        }
        Err(e) => Err(e).wrap_err_with(|| format!("Failed to read {}", path)),
    }
}
