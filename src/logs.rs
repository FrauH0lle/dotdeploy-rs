//! This module defines the logging facilities, using log and simplelog.
//!
//! Logs will displayed in the terminal and written to log files. By default, only the log files of
//! the last 10 runs will be kept.

use crate::TERMINAL_LOCK;
use chrono::Local;
use color_eyre::{eyre::WrapErr, Result};
use std::path::{Path, PathBuf};
use std::{fs, io};
use tracing::{debug, instrument, Level};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_error::ErrorLayer;
use tracing_subscriber::filter::FilterFn;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{fmt, EnvFilter};

/// Initialize the logging system with both terminal and file output.
///
/// This function sets up a combined logging system that writes to both the terminal and a log file.
/// It also handles log rotation to keep only the most recent logs.
///
/// # Arguments
///
/// * `verbosity` - Controls the log level (0 = Info, 1 = Debug, 2 = Trace)
///
/// # Returns
///
/// Returns `Ok(())` if logging was successfully initialized, or an error if something failed.
#[instrument]
pub(crate) fn init_logging(verbosity: u8) -> Result<WorkerGuard> {
    // Convert verbosity level to Level
    let level = match verbosity {
        0 => Level::INFO,
        1 => Level::DEBUG,
        _ => Level::TRACE,
    };

    // Set up file appender
    let log_dir = get_log_dir()?;
    let timestamp = Local::now().format("%Y%m%d_%H%M%S");

    // Create file appender
    let file_appender =
        tracing_appender::rolling::never(&log_dir, format!("dotdeploy_{}.log", timestamp));
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    
    // Create terminal layer, INFO level only
    let terminal_info_layer = fmt::layer()
        .with_target(if level >= Level::DEBUG { true } else { false })
        .with_writer(move || -> Box<dyn io::Write> {
            let _guard = TERMINAL_LOCK.read();
            Box::new(std::io::stdout())
        })
        .with_thread_ids(if level >= Level::DEBUG { true } else { false })
        .with_thread_names(if level >= Level::DEBUG { true } else { false })
        .with_file(if level >= Level::DEBUG { true } else { false })
        .with_line_number(if level >= Level::DEBUG { true } else { false })
        // Time format similar to 2025-02-01T22:15:57.427
        .with_timer(tracing_subscriber::fmt::time::ChronoLocal::new(
            "%FT%X%.3f".to_string(),
        ))
        .with_filter(FilterFn::new(|metadata| metadata.level() == &Level::INFO));

    // Create terminal layer, all other levels
    let terminal_non_info_layer = fmt::layer()
        .with_target(true)
        .with_writer(move || -> Box<dyn io::Write> {
            let _guard = TERMINAL_LOCK.read();
            Box::new(std::io::stdout())
        })
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_file(true)
        .with_line_number(true)
        // Time format similar to 2025-02-01T22:15:57.427
        .with_timer(tracing_subscriber::fmt::time::ChronoLocal::new(
            "%FT%X%.3f".to_string(),
        ))
        .with_filter(EnvFilter::from_env("DOD_VERBOSE").add_directive(level.into()))
        .with_filter(FilterFn::new(|metadata| metadata.level() != &Level::INFO));

    // Create file layer
    let file_layer = fmt::layer()
        .with_target(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_file(true)
        .with_line_number(true)
        // Do not print ANSI escape codes into the log file
        .with_ansi(false)
        .with_writer(non_blocking)
        .with_filter(EnvFilter::from_env("DOD_VERBOSE").add_directive(level.into()));

    // Combine layers and set as global subscriber
    tracing_subscriber::registry()
        .with(terminal_info_layer)
        .with(terminal_non_info_layer)
        .with(file_layer)
        .with(ErrorLayer::default())
        .init();

    // Perform log rotation
    rotate_logs(&log_dir)?;

    Ok(guard)
}

/// Get the directory where log files should be stored.
///
/// Uses XDG_DATA_HOME/dotdeploy/logs if available, otherwise defaults to
/// ~/.local/share/dotdeploy/logs
fn get_log_dir() -> Result<PathBuf> {
    let log_dir = if let Ok(data_dir) = std::env::var("XDG_DATA_HOME") {
        debug!(?data_dir, "Using XDG_DATA_HOME for log directory");
        PathBuf::from(data_dir).join("dotdeploy").join("logs")
    } else {
        let home =
            std::env::var("HOME").wrap_err("Failed to get HOME directory for log location")?;
        debug!(?home, "Using HOME directory for log location");
        PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("dotdeploy")
            .join("logs")
    };

    // Create directory if it doesn't exist
    fs::create_dir_all(&log_dir)
        .wrap_err_with(|| format!("Failed to create log directory at {:?}", log_dir))?;
    debug!(?log_dir, "Log directory created/confirmed");
    Ok(log_dir)
}

/// Rotate log files, keeping only the most recent ones.
///
/// # Arguments
///
/// * `log_dir` - Directory containing the log files
///
/// # Returns
///
/// Returns `Ok(())` if rotation was successful, or an error if file operations failed.
#[instrument(skip(log_dir))]
fn rotate_logs<P: AsRef<Path>>(log_dir: P) -> Result<()> {
    const MAX_LOGS: usize = 10;

    debug!("Starting log rotation");
    // Get all log files
    let mut log_files: Vec<_> = fs::read_dir(&log_dir)
        .wrap_err_with(|| format!("Failed to read log directory {:?}", log_dir.as_ref()))?
        // Filter out entries which could not be read (should be zero).
        .filter_map(|entry| entry.ok())
        // Filter out entries with extensions other than .log
        .filter_map(|path| {
            if path.path().extension().map_or(false, |ext| ext == "log") {
                Some(path)
            } else {
                None
            }
        })
        .collect();

    // Sort by file name and reverse order -> newest first
    log_files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    log_files.reverse();

    debug!(total_logs = log_files.len(), "Found log files");

    // Remove old logs
    for old_log in log_files.iter().skip(MAX_LOGS) {
        let path = old_log.path();
        debug!(?path, "Removing old log file");
        fs::remove_file(&path)
            .wrap_err_with(|| format!("Failed to remove old log file {:?}", &path))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use tempfile::tempdir;

    #[test]
    fn test_log_rotation() -> Result<()> {
        let temp_dir = tempdir()?;
        // Create more than MAX_LOGS log files
        for i in 0..10 {
            // Simulate different timestamps
            let log_file = temp_dir
                .path()
                .join(format!("dotdeploy_20241028_21441{}.log", i));
            File::create(&log_file)?;
        }
        for i in 0..5 {
            // Simulate different timestamps
            let log_file = temp_dir
                .path()
                .join(format!("dotdeploy_20241028_21442{}.log", i));
            File::create(&log_file)?;
        }

        // Perform rotation
        rotate_logs(&temp_dir.path())?;

        // --
        // Check number of remaining files
        let remaining_logs: Vec<_> = fs::read_dir(&temp_dir)?
            .filter_map(|entry| entry.ok())
            .collect();

        assert_eq!(remaining_logs.len(), 10, "Should keep exactly 10 log files");

        // --
        // Verify that remaining files are as expected
        assert_eq!(
            remaining_logs[0].path(),
            PathBuf::from(temp_dir.path().join("dotdeploy_20241028_214415.log")),
            "First remaining file should be dotdeploy_20241028_214415.log"
        );
        assert_eq!(
            remaining_logs[remaining_logs.len() - 1].path(),
            PathBuf::from(temp_dir.path().join("dotdeploy_20241028_214424.log")),
            "Last remaining file should be dotdeploy_20241028_214424.log"
        );

        Ok(())
    }

    #[test]
    fn test_get_log_dir() -> Result<()> {
        // --
        // Test with XDG_DATA_HOME

        // Store old value
        let old_env = std::env::var("XDG_DATA_HOME")?;

        let temp_dir = tempdir()?;

        std::env::set_var("XDG_DATA_HOME", temp_dir.path());
        let log_dir = get_log_dir()?;
        assert_eq!(
            log_dir,
            PathBuf::from(temp_dir.path().join("dotdeploy").join("logs")),
            "Should use XDG_DATA_HOME when available"
        );
        // Restore old value
        std::env::set_var("XDG_DATA_HOME", old_env);

        // --
        // Test with HOME

        // Store old value
        let old_env = std::env::var("XDG_DATA_HOME")?;

        let temp_dir = tempdir()?;

        std::env::remove_var("XDG_DATA_HOME");

        let old_env_home = std::env::var("HOME")?;
        std::env::set_var("HOME", temp_dir.path());
        let log_dir = get_log_dir()?;
        assert_eq!(
            log_dir,
            PathBuf::from(
                temp_dir
                    .path()
                    .join(".local")
                    .join("share")
                    .join("dotdeploy")
                    .join("logs")
            ),
            "Should use HOME when XDG_DATA_HOME is not available"
        );
        // Restore old values
        std::env::set_var("XDG_DATA_HOME", old_env);
        std::env::set_var("HOME", old_env_home);

        Ok(())
    }
}
