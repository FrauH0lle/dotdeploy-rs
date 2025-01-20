//! This module defines the logging facilities, using log and simplelog.
//!
//! Logs will displayed in the terminal and written to log files. By default, only the log files of
//! the last 10 runs will be kept.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Local;
use simplelog::{
    ColorChoice, CombinedLogger, ConfigBuilder, LevelFilter, LevelPadding, SharedLogger,
    TermLogger, TerminalMode, WriteLogger,
};

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
pub(crate) fn init_logging(verbosity: u8) -> Result<()> {
    // Convert verbosity level to LevelFilter
    let level = match verbosity {
        0 => LevelFilter::Info,
        1 => LevelFilter::Debug,
        _ => LevelFilter::Trace,
    };

    // Create logging config
    let config = ConfigBuilder::new()
        .set_time_level(LevelFilter::Debug)
        .set_location_level(LevelFilter::Debug)
        .set_target_level(LevelFilter::Debug)
        .set_thread_level(LevelFilter::Debug)
        .set_level_padding(LevelPadding::Left)
        .add_filter_allow("dotdeploy".to_string())
        .build();

    // Determine log file path
    let log_dir = get_log_dir()?;
    let timestamp = Local::now().format("%Y%m%d_%H%M%S");
    let log_file = log_dir.join(format!("dotdeploy_{}.log", timestamp));

    // Perform log rotation before creating new log
    rotate_logs(&log_dir)?;

    // Create vector of loggers
    let loggers: Vec<Box<dyn SharedLogger>> = vec![
        // Terminal logger
        TermLogger::new(
            level,
            config.clone(),
            TerminalMode::Mixed,
            ColorChoice::Auto,
        ),
        // File logger
        WriteLogger::new(
            level,
            config,
            fs::File::create(&log_file)
                .with_context(|| format!("Failed to create log file at {:?}", log_file))?,
        ),
    ];

    // Initialize the combined logger
    CombinedLogger::init(loggers).context("Failed to initialize logging")?;

    Ok(())
}

/// Get the directory where log files should be stored.
///
/// Uses XDG_DATA_HOME/dotdeploy/logs if available, otherwise defaults to
/// ~/.local/share/dotdeploy/logs
fn get_log_dir() -> Result<PathBuf> {
    let log_dir = if let Ok(data_dir) = std::env::var("XDG_DATA_HOME") {
        PathBuf::from(data_dir).join("dotdeploy").join("logs")
    } else {
        let home =
            std::env::var("HOME").context("Failed to get HOME directory for log location")?;
        PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("dotdeploy")
            .join("logs")
    };

    // Create directory if it doesn't exist
    fs::create_dir_all(&log_dir)
        .with_context(|| format!("Failed to create log directory at {:?}", log_dir))?;

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
fn rotate_logs<P: AsRef<Path>>(log_dir: P) -> Result<()> {
    const MAX_LOGS: usize = 10;

    // Get all log files
    let mut log_files: Vec<_> = fs::read_dir(&log_dir)
        .with_context(|| format!("Failed to read log directory {:?}", log_dir.as_ref()))?
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

    // Remove old logs
    for old_log in log_files.iter().skip(MAX_LOGS) {
        fs::remove_file(old_log.path())
            .with_context(|| format!("Failed to remove old log file {:?}", old_log.path()))?;
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

        // Check number of remaining files
        let remaining_logs: Vec<_> = fs::read_dir(&temp_dir)?
            .filter_map(|entry| entry.ok())
            .collect();

        assert_eq!(remaining_logs.len(), 10, "Should keep exactly 10 log files");
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

        //
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
