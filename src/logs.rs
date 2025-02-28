//! This module defines the logging facilities using tracing and tracing-subscriber.
//!
//! Logs are displayed in the terminal and written to log files. By default, only the log files of
//! the last 15 runs are kept.

use chrono::Local;
use color_eyre::eyre::OptionExt;
use color_eyre::{Result, eyre::WrapErr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::{fs, io};
use tracing::{Level, debug, instrument};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_error::ErrorLayer;
use tracing_subscriber::filter::FilterFn;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, fmt};

// -------------------------------------------------------------------------------------------------
// Logger
// -------------------------------------------------------------------------------------------------

/// Central logging facility for dotdeploy that manages both terminal and file output.
#[derive(Debug, Clone)]
pub(crate) struct Logger {
    /// Synchronization for terminal output.
    ///
    /// This ensures that sudo password prompts and similar terminal interactions don't overlap and
    /// confuse the user, especially in concurrent operations.
    pub(crate) terminal_lock: Arc<RwLock<()>>,
    /// Logging level
    pub(crate) verbosity: Level,
    /// Maximum number of log files to retain
    pub(crate) max_logs: usize,
    /// Directory where log files are stored
    pub(crate) log_dir: PathBuf,
}

impl Logger {
    /// Initialize the logging system with both terminal and file output.
    ///
    /// This function sets up a combined logging system that writes to both the terminal and a log file.
    /// It also handles log rotation to keep only the most recent logs.
    ///
    /// # Arguments
    /// * `verbosity` - Controls the log level (0 = Info, 1 = Debug, 2 = Trace)
    ///
    /// # Errors
    /// Returns an error if something in the [`Logger`] initialization fails.
    #[instrument]
    pub(crate) fn start(&self) -> Result<WorkerGuard> {
        let level = self.verbosity;

        // Set up file appender
        let timestamp = Local::now().format("%Y%m%d_%H%M%S");

        // Create file appender with timestamp format YYYYMMDD_HHMMSS
        let file_appender =
            tracing_appender::rolling::never(&self.log_dir, format!("dotdeploy_{}.log", timestamp));
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

        let lock = Arc::clone(&self.terminal_lock);
        // Create terminal layer, INFO level only
        let terminal_info_layer = fmt::layer()
            .compact()
            .with_target(level >= Level::TRACE)
            .with_writer(move || -> Box<dyn io::Write> {
                let _guard = lock.read();
                Box::new(std::io::stdout())
            })
            .with_thread_ids(level >= Level::TRACE)
            .with_thread_names(level >= Level::TRACE)
            .with_file(level >= Level::TRACE)
            .with_line_number(level >= Level::TRACE)
            // Time format similar to 2025-02-01T22:15:57.427
            .with_timer(tracing_subscriber::fmt::time::ChronoLocal::new(
                "%FT%X%.3f".to_string(),
            ))
            .with_filter(FilterFn::new(move |metadata| {
                metadata.level() == &Level::INFO
                    && (level == Level::TRACE
                        || (metadata.target().starts_with("dotdeploy")
                            && !metadata.target().starts_with("dotdeploy_logfile")))
            }));

        let lock = Arc::clone(&self.terminal_lock);
        // Create terminal layer, all other levels
        let terminal_non_info_layer = fmt::layer()
            .compact()
            .with_target(level >= Level::TRACE)
            .with_writer(move || -> Box<dyn io::Write> {
                let _guard = lock.read();
                Box::new(std::io::stdout())
            })
            .with_thread_ids(level >= Level::TRACE)
            .with_thread_names(level >= Level::TRACE)
            .with_file(level >= Level::TRACE)
            .with_line_number(level >= Level::TRACE)
            // Time format similar to 2025-02-01T22:15:57.427
            .with_timer(tracing_subscriber::fmt::time::ChronoLocal::new(
                "%FT%X%.3f".to_string(),
            ))
            .with_filter(EnvFilter::from_env("DOD_VERBOSE").add_directive(level.into()))
            .with_filter(FilterFn::new(move |metadata| {
                metadata.level() != &Level::INFO
                    && (level == Level::TRACE
                        || (metadata.target().starts_with("dotdeploy")
                            && !metadata.target().starts_with("dotdeploy_logfile")))
            }));

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
            .with(file_layer)
            .with(terminal_info_layer)
            .with(terminal_non_info_layer)
            .with(ErrorLayer::default())
            .init();

        // Perform log rotation
        self.rotate_logs()?;

        Ok(guard)
    }

    /// Rotate log files, keeping only the most recent ones.
    ///
    /// # Errors
    /// Returns an error if:
    /// * Log directory cannot be read
    /// * Old log files cannot be removed
    #[instrument]
    fn rotate_logs(&self) -> Result<()> {
        debug!("Starting log rotation");
        // Get all log files
        let mut log_files: Vec<_> = fs::read_dir(&self.log_dir)
            .wrap_err_with(|| format!("Failed to read log directory {:?}", &self.log_dir))?
            // Filter out entries which could not be read (should be zero).
            .filter_map(|entry| entry.ok())
            // Filter out entries with extensions other than .log
            .filter_map(|path| {
                if path.path().extension().is_some_and(|ext| ext == "log") {
                    Some(path)
                } else {
                    None
                }
            })
            .collect();

        // Sort by file name and reverse order -> newest first
        log_files.sort_by_key(|a| a.file_name());
        log_files.reverse();

        debug!(total_logs = log_files.len(), "Found log files");

        // Remove old logs
        for old_log in log_files.iter().skip(self.max_logs) {
            let path = old_log.path();
            debug!(?path, "Removing old log file");
            fs::remove_file(&path)
                .wrap_err_with(|| format!("Failed to remove old log file {:?}", &path))?;
        }

        Ok(())
    }
}

// -------------------------------------------------------------------------------------------------
// LoggerBuilder
// -------------------------------------------------------------------------------------------------

/// Default maximum number of log files to retain
pub(crate) const DEFAULT_MAX_LOGS: usize = 15;

/// Builder for configuring and creating a [`Logger`] instance.
///
/// Provides a fluent interface for configuring:
/// * Verbosity level
/// * Log retention policy
/// * Log storage location
///
/// * `verbosity` - Controls detail level (0=Info, 1=Debug, 2+=Trace)
/// * `max_logs` - Maximum historical logs to retain (default: 15)
/// * `log_dir` - Storage location for log files
#[derive(Debug, Default)]
pub(crate) struct LoggerBuilder {
    /// The logging verbosity level (INFO, DEBUG, or TRACE)
    verbosity: Option<Level>,
    /// Maximum number of log files to keep during rotation
    max_logs: Option<usize>,
    /// Directory where log files will be stored
    log_dir: Option<PathBuf>,
}

impl LoggerBuilder {
    /// Creates a new LoggerBuilder with default settings.
    pub(crate) fn new() -> Self {
        LoggerBuilder::default()
    }

    /// Sets the verbosity level for logging.
    ///
    /// # Arguments
    /// * `verbosity` - Verbosity level (0 = INFO, 1 = DEBUG, 2+ = TRACE)
    pub(crate) fn with_verbosity(&mut self, verbosity: u8) -> &mut Self {
        let new = self;
        new.verbosity = Some(match verbosity {
            0 => Level::INFO,
            1 => Level::DEBUG,
            _ => Level::TRACE,
        });
        new
    }

    /// Sets the maximum number of log files to retain during rotation.
    ///
    /// # Arguments
    /// * `count` - Maximum number of log files to keep
    pub(crate) fn with_max_logs(&mut self, count: usize) -> &mut Self {
        let new = self;
        new.max_logs = Some(count);
        new
    }

    /// Sets the directory where log files will be stored.
    ///
    /// # Arguments
    /// * `dir` - Path to the directory for storing log files
    pub(crate) fn with_log_dir(&mut self, dir: &Path) -> &mut Self {
        let new = self;
        new.log_dir = Some(dir.into());
        new
    }

    /// Builds and returns a new [`Logger`] instance with the configured settings.
    ///
    /// # Errors
    /// Returns an error if required settings are missing or if log directory creation fails.
    pub(crate) fn build(&self) -> Result<Logger> {
        Ok(Logger {
            terminal_lock: Arc::new(RwLock::new(())),
            verbosity: self.verbosity.ok_or_eyre("Verbosity level undefined")?,
            max_logs: self.max_logs.unwrap_or(DEFAULT_MAX_LOGS),
            log_dir: match self.log_dir {
                Some(ref value) => Clone::clone(value),
                None => get_default_log_dir()?,
            },
        })
    }
}

/// Get the directory where log files should be stored.
///
/// Uses XDG_DATA_HOME/dotdeploy/logs if available, otherwise defaults to
/// ~/.local/share/dotdeploy/logs
#[instrument]
pub(crate) fn get_default_log_dir() -> Result<PathBuf> {
    let log_dir = dirs::data_dir()
        .ok_or_eyre("Failed to determine user's local data dir")?
        .join("dotdeploy")
        .join("logs");

    // Create directory if it doesn't exist
    fs::create_dir_all(&log_dir)
        .wrap_err_with(|| format!("Failed to create log directory at {:?}", log_dir))?;
    debug!(?log_dir, "Log directory created/confirmed");
    Ok(log_dir)
}

// -------------------------------------------------------------------------------------------------
// Output to log file macro
// -------------------------------------------------------------------------------------------------

/// Logs command output with appropriate level and formatting
///
/// This macro logs command output (stdout/stderr) only when the stream contains data. Automatically
/// determines output type (stdout/stderr) based on input stream name.
///
/// # Arguments
/// * `stream` - The output stream (stdout/stderr) to log
/// * `cmd` - The command that generated the output (for context)
/// * `log_level` - The logging level to use (info/error)
macro_rules! log_output {
    ($stream:expr, $label:expr, $cmd:expr, $log_level:ident) => {
        // Only log non-empty outputs
        if !$stream.is_empty() {
            $log_level!(
                // Only log to log file
                target: "dotdeploy_logfile",
                "{} from `{}`:\n{}",
                // Output label
                $label,
                // Command name
                $cmd,
                 // Convert bytes to string
                std::str::from_utf8(&$stream)
                    .expect("Bytes should be valid utf8")
            );
        }
    };
}
pub(crate) use log_output;


// -------------------------------------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------------------------------------

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
        for i in 0..10 {
            // Simulate different timestamps
            let log_file = temp_dir
                .path()
                .join(format!("dotdeploy_20241028_21442{}.log", i));
            File::create(&log_file)?;
        }

        let logger = LoggerBuilder::new()
            .with_verbosity(1)
            .with_log_dir(temp_dir.path())
            .build()?;

        // Perform rotation
        logger.rotate_logs()?;

        // --
        // * Check number of remaining files
        let mut remaining_logs: Vec<_> = fs::read_dir(&temp_dir)?
            .filter_map(|entry| entry.ok())
            .collect();
        remaining_logs.sort_by_key(|a| a.file_name());

        assert_eq!(
            remaining_logs.len(),
            15,
            "Should keep exactly 15 log files after rotation"
        );

        // --
        // Verify that remaining files are as expected
        assert_eq!(
            remaining_logs[0].path(),
            temp_dir.path().join("dotdeploy_20241028_214415.log"),
            "Should preserve chronological order when retaining logs"
        );
        assert_eq!(
            remaining_logs[remaining_logs.len() - 1].path(),
            temp_dir.path().join("dotdeploy_20241028_214429.log"),
            "Should retain the most recent log files during rotation"
        );

        Ok(())
    }

    #[test]
    fn test_get_default_log_dir() -> Result<()> {
        // --
        // * Test with XDG_DATA_HOME

        let temp_dir = tempdir()?;

        temp_env::with_var("XDG_DATA_HOME", Some(temp_dir.path()), || -> Result<()> {
            let log_dir = get_default_log_dir()?;
            assert_eq!(
                log_dir,
                temp_dir.path().join("dotdeploy").join("logs"),
                "Should use XDG_DATA_HOME environment variable when available"
            );
            Ok(())
        })?;

        // --
        // * Test with HOME

        let temp_dir = tempdir()?;

        temp_env::with_vars(
            [("XDG_DATA_HOME", None), ("HOME", Some(temp_dir.path()))],
            || -> Result<()> {
                let log_dir = get_default_log_dir()?;
                assert_eq!(
                    log_dir,
                    temp_dir
                        .path()
                        .join(".local")
                        .join("share")
                        .join("dotdeploy")
                        .join("logs"),
                    "Should fall back to HOME environment variable when XDG_DATA_HOME is not available"
                );
                Ok(())
            },
        )?;

        Ok(())
    }
}
