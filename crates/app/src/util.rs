//! Tiny generic helpers shared across modules.

use sha2::{Digest, Sha256};

/// Lowercase hex SHA-256 of arbitrary bytes.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Lowercase hex SHA-256 of a UTF-8 string.
pub fn sha256_hex_str(text: &str) -> String {
    sha256_hex(text.as_bytes())
}

/// Canonical hash of a JSON value. `serde_json::Map` is a `BTreeMap` by default,
/// so object keys are serialised in sorted order and the hash is stable.
pub fn json_hash(value: &serde_json::Value) -> String {
    sha256_hex_str(&serde_json::to_string(value).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha_is_stable_and_sensitive() {
        assert_eq!(sha256_hex_str("abc"), sha256_hex_str("abc"));
        assert_ne!(sha256_hex_str("abc"), sha256_hex_str("abd"));
        // Known SHA-256 of "abc".
        assert_eq!(
            sha256_hex_str("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn json_hash_ignores_key_order() {
        let a: serde_json::Value = serde_json::json!({"b": 1, "a": 2});
        let b: serde_json::Value = serde_json::json!({"a": 2, "b": 1});
        assert_eq!(json_hash(&a), json_hash(&b));
    }
}
