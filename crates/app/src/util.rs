//! Tiny generic helpers shared across modules.

use std::collections::BTreeSet;

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

/// Write a shipped prompt file into a project snapshot unless the user owns it.
///
/// Prompts are user data, so an existing file is kept — except one still
/// byte-identical to a default an earlier release shipped (its SHA-256 is listed in
/// `shipped`): nobody edited it, so it follows the current default instead of
/// pinning the project to an old prompt forever.
pub async fn ensure_prompt_file(
    path: &std::path::Path,
    content: &str,
    shipped: &[&str],
) -> crate::error::Result<()> {
    let stale = match tokio::fs::read_to_string(path).await {
        Ok(current) => current != content && shipped.contains(&sha256_hex_str(&current).as_str()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => return Err(error.into()),
    };
    if stale {
        tokio::fs::write(path, content).await?;
    }
    Ok(())
}

/// Canonical hash of a JSON value. `serde_json::Map` is a `BTreeMap` by default,
/// so object keys are serialised in sorted order and the hash is stable.
pub fn json_hash(value: &serde_json::Value) -> String {
    sha256_hex_str(&serde_json::to_string(value).unwrap_or_default())
}

/// Trim and cap at `max` characters (ellipsis included). Used for every value
/// that enters the project profile or the glossary.
pub fn clamp_chars(value: &str, max: usize) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut capped: String = trimmed.chars().take(max - 1).collect();
    capped = capped.trim_end().to_string();
    capped.push('…');
    capped
}

/// Join whitespace-separated words with single spaces and cap the count.
pub fn clamp_words(value: &str, max: usize) -> String {
    let words: Vec<&str> = value.split_whitespace().collect();
    if words.len() <= max {
        return words.join(" ");
    }
    let mut capped = words[..max].join(" ");
    capped.push('…');
    capped
}

/// Trim, drop empties, dedupe case-insensitively, cap count and length.
pub fn clamp_list(items: &[String], max_items: usize, max_chars: usize) -> Vec<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::new();
    for item in items {
        let value = clamp_chars(item, max_chars);
        if value.is_empty() || !seen.insert(value.to_lowercase()) {
            continue;
        }
        out.push(value);
        if out.len() >= max_items {
            break;
        }
    }
    out
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

    #[test]
    fn clamps_trim_cap_and_dedupe() {
        assert_eq!(clamp_chars("  hello  ", 10), "hello");
        assert_eq!(clamp_chars("abcdef", 4), "abc…");
        assert_eq!(clamp_chars("abcdef", 0), "");
        assert!(clamp_chars("abcdef", 4).chars().count() <= 4);
        assert_eq!(clamp_words("one  two\nthree", 10), "one two three");
        assert_eq!(clamp_words("one two three", 2), "one two…");
        let items = vec![
            "King".to_string(),
            "king".to_string(),
            " ".to_string(),
            "queen".to_string(),
        ];
        assert_eq!(clamp_list(&items, 5, 20), vec!["King", "queen"]);
        assert_eq!(clamp_list(&items, 1, 20), vec!["King"]);
    }
}
