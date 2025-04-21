use color_eyre::Section;
use color_eyre::eyre::{Report, eyre};
use thiserror::Error;

// This error is used to collect multiple eyre Reports into one
#[derive(Debug, Error)]
#[error("{0}")]
pub(crate) struct StrError(pub(crate) String);

/// Combine multiple results into either a success vector or aggregated error report
///
/// * `results` - Vector of results to combine
///
/// # Errors
///
/// Returns aggregated error report containing all individual errors if any results were Err
pub(crate) fn join_errors<T>(results: Vec<Result<T, Report>>) -> Result<Vec<T>, Report> {
    let mut values = Vec::with_capacity(results.len());
    let mut errors = Vec::new();

    // Separate successes and errors in single pass
    for result in results {
        match result {
            Ok(value) => values.push(value),
            Err(err) => errors.push(err),
        }
    }

    if errors.is_empty() {
        Ok(values)
    } else {
        // Aggregate all errors into single report
        let combined = errors
            .into_iter()
            .fold(eyre!("Encountered multiple errors"), |report, e| {
                report.with_error(|| StrError(format!("{:?}", e)))
            });
        Err(combined)
    }
}
