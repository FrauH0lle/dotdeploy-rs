//! Common utility functions module.
//!
//! This module provides utility functions that are commonly used across the project. Currently, it
//! includes functionality for user interaction, specifically for asking yes/no questions to the
//! user via the command line.

use std::io::{stdin, stdout, Write};

/// Asks the user for a yes/no confirmation.
///
/// This function prompts the user with a given question and waits for a yes/no response. It
/// repeatedly asks until a valid response is given. The function considers 'y', 'Y', or an empty
/// input (default to no) as valid responses.
///
/// # Arguments
///
/// * `prompt` - A string slice that holds the question to be asked to the user.
///
/// # Returns
///
/// * `bool` - Returns `true` if the user responds with 'y' or 'Y', `false` otherwise (including for
///   an empty input).
///
/// # Examples
///
/// ```
/// let result = ask_boolean("Do you want to continue? [y/N] ");
/// if result {
///     println!("User chose to continue");
/// } else {
///     println!("User chose not to continue");
/// }
/// ```
pub(crate) fn ask_boolean(prompt: &str) -> bool {
    // Initialize buffer with a non-empty string to enter the loop at least once
    let mut buf = String::from("a");
    // Continue asking until a valid response is given
    while !(buf.to_lowercase().starts_with('y')
        || buf.to_lowercase().starts_with('n')
        || buf.is_empty())
    {
        // Print the prompt
        eprintln!("{}", prompt);
        // Clear the buffer for new input
        buf.clear();
        // Ensure the prompt is immediately visible
        stdout().flush().expect("Failed to flush stdout");
        // Read user input
        stdin()
            .read_line(&mut buf)
            .expect("Failed to read line from stdin");
    }

    // Return true if the response starts with 'y' or 'Y', false otherwise
    // Note: An empty input (just pressing Enter) defaults to 'no'
    buf.to_lowercase().starts_with('y')
}
