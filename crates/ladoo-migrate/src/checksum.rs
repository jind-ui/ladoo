//! SHA-256 checksum computation for migration SQL.
//!
//! The checksum covers only the `@up` block content (after directives,
//! before `@down`). This is a security invariant: changing the up SQL
//! changes the checksum, and the engine detects the mismatch.

use sha2::{Digest, Sha256};

/// Compute the SHA-256 hex digest of the given SQL string.
///
/// Used to fingerprint the `@up` block of each migration file.
/// Trailing whitespace is preserved — the checksum is over the exact
/// content as parsed.
///
/// # Examples
///
/// ```
/// use ladoo_migrate::checksum::compute_checksum;
///
/// let hash = compute_checksum("CREATE TABLE users (id INT);");
/// assert_eq!(hash.len(), 64); // SHA-256 produces 64 hex chars
/// ```
pub fn compute_checksum(sql: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(sql.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn produces_64_char_hex_string() {
        let hash = compute_checksum("CREATE TABLE users (id INT);");
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn same_input_same_output() {
        let a = compute_checksum("SELECT 1;");
        let b = compute_checksum("SELECT 1;");
        assert_eq!(a, b);
    }

    #[test]
    fn different_input_different_output() {
        let a = compute_checksum("SELECT 1;");
        let b = compute_checksum("SELECT 2;");
        assert_ne!(a, b);
    }

    #[test]
    fn empty_string_produces_valid_hash() {
        let hash = compute_checksum("");
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn whitespace_matters() {
        let a = compute_checksum("SELECT 1;");
        let b = compute_checksum("SELECT  1;");
        assert_ne!(a, b);
    }
}
