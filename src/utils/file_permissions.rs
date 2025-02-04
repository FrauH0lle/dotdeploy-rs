//! File permissions utility module.
//!
//! This module provides utility functions for working with file permissions, user IDs, and group
//! IDs. It includes functionality to convert between different representations of permissions and
//! to resolve user and group names to their respective numeric IDs.

use color_eyre::{eyre::WrapErr, Result};

/// Converts permissions from u32 to string format.
///
/// This function takes a u32 representing file permissions in octal format and converts it to a
/// three-digit string representation.
///
/// # Arguments
///
/// * `p` - The permissions as a u32 in octal format.
///
/// # Returns
///
/// * `Result<String>` - The permissions as a three-digit string, or an error if conversion fails.
///
/// # Examples
///
/// ```
/// # use anyhow::Result;
/// # fn main() -> Result<()> {
/// assert_eq!(perms_int_to_str(0o644)?, "644");
/// # Ok(())
/// # }
/// ```
pub(crate) fn perms_int_to_str(p: u32) -> Result<String> {
    let s = format!("{:o}", p);
    // Take only the last three digits of the conversion result
    let split_pos = s.char_indices().nth_back(2).unwrap().0;
    Ok(s[split_pos..].to_string())
}

/// Converts permissions from string to u32 format.
///
/// This function takes a string representing file permissions and converts it to a u32 in octal
/// format.
///
/// # Arguments
///
/// * `p` - The permissions as a string (e.g., "644").
///
/// # Returns
///
/// * `Result<u32>` - The permissions as a u32 in octal format, or an error if conversion fails.
///
/// # Examples
///
/// ```
/// # use anyhow::Result;
/// # fn main() -> Result<()> {
/// assert_eq!(perms_str_to_int("644")?, 0o644);
/// # Ok(())
/// # }
/// ```
pub(crate) fn perms_str_to_int<S: AsRef<str>>(p: S) -> Result<u32> {
    u32::from_str_radix(p.as_ref(), 8).wrap_err("Failed to convert permission string to u32")
}

/// Converts a username to its corresponding user ID (UID).
///
/// This function looks up the UID for a given username using the system's user database.
///
/// # Arguments
///
/// * `u` - The username to look up.
///
/// # Returns
///
/// * `Result<u32>` - The UID corresponding to the username, or an error if the lookup fails.
///
/// # Examples
///
/// ```
/// # use anyhow::Result;
/// # fn main() -> Result<()> {
/// assert_eq!(user_to_uid("root")?, 0);
/// # Ok(())
/// # }
/// ```
pub(crate) fn user_to_uid<S: AsRef<str>>(u: S) -> Result<u32> {
    Ok(nix::unistd::User::from_name(u.as_ref())
        .unwrap()
        .unwrap()
        .uid
        .as_raw())
}

/// Converts a group name to its corresponding group ID (GID).
///
/// This function looks up the GID for a given group name using the system's group database.
///
/// # Arguments
///
/// * `u` - The group name to look up.
///
/// # Returns
///
/// * `Result<u32>` - The GID corresponding to the group name, or an error if the lookup fails.
///
/// # Examples
///
/// ```
/// # use anyhow::Result;
/// # fn main() -> Result<()> {
/// assert_eq!(group_to_gid("root")?, 0);
/// # Ok(())
/// # }
/// ```
pub(crate) fn group_to_gid<S: AsRef<str>>(u: S) -> Result<u32> {
    Ok(nix::unistd::Group::from_name(u.as_ref())
        .unwrap()
        .unwrap()
        .gid
        .as_raw())
}

//
// Tests

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_perms_int_to_str() -> Result<()> {
        assert_eq!(perms_int_to_str(33188)?, "644");
        assert_eq!(perms_int_to_str(0o644)?, "644");
        assert_eq!(perms_int_to_str(0o600)?, "600");
        Ok(())
    }

    #[tokio::test]
    async fn test_perms_str_to_int() -> Result<()> {
        assert_eq!(perms_str_to_int("644")?, 0o644);
        Ok(())
    }

    #[tokio::test]
    async fn test_user_to_uid() -> Result<()> {
        assert_eq!(user_to_uid("root")?, 0);
        Ok(())
    }

    #[tokio::test]
    async fn test_group_to_gid() -> Result<()> {
        assert_eq!(group_to_gid("root")?, 0);
        Ok(())
    }
}
