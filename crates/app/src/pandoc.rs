//! Pandoc bridge: book metadata (`metadata.yaml`) plus the driver that calls the
//! sidecar's `pandoc_build` method.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;
use crate::sidecar::{PandocParams, PandocResult, PandocUnit, SidecarClient};

/// Pandoc metadata for a book build. Rendered to `metadata.yaml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BookMetadata {
    pub title: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub publisher: Option<String>,
    #[serde(default)]
    pub identifier: Option<String>,
    /// Any additional key/value pairs (front matter from the source document).
    #[serde(flatten, default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, Value>,
}

impl BookMetadata {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Default::default()
        }
    }

    /// Render to a `metadata.yaml` document.
    pub fn to_yaml(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("title: {}\n", yaml_scalar(&self.title)));
        push_opt(&mut out, "author", self.author.as_deref());
        push_opt(&mut out, "language", self.language.as_deref());
        push_opt(&mut out, "date", self.date.as_deref());
        push_opt(&mut out, "publisher", self.publisher.as_deref());
        push_opt(&mut out, "identifier", self.identifier.as_deref());
        for (key, value) in &self.extra {
            let rendered = match value {
                Value::String(s) => yaml_scalar(s),
                Value::Null => "null".to_string(),
                other => other.to_string(),
            };
            out.push_str(&format!("{}: {}\n", yaml_scalar(key), rendered));
        }
        out
    }
}

fn push_opt(out: &mut String, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        out.push_str(&format!("{key}: {}\n", yaml_scalar(value)));
    }
}

/// Quote a scalar when YAML would otherwise misinterpret it.
pub fn yaml_scalar(value: &str) -> String {
    if value.is_empty() {
        return "\"\"".to_string();
    }
    let needs_quoting = value.chars().any(|c| {
        matches!(
            c,
            ':' | '#'
                | '\n'
                | '"'
                | '\''
                | '['
                | ']'
                | '{'
                | '}'
                | ','
                | '&'
                | '*'
                | '!'
                | '|'
                | '>'
                | '%'
                | '@'
                | '`'
        )
    }) || value.starts_with(' ')
        || value.ends_with(' ')
        || value.starts_with('-')
        || value.starts_with('?');
    if needs_quoting {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

/// Write `metadata.yaml` into `dir`, returning its path.
pub fn write_metadata_yaml(dir: &Path, metadata: &BookMetadata) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join("metadata.yaml");
    std::fs::write(&path, metadata.to_yaml())?;
    Ok(path)
}

/// Drives Pandoc builds through the sidecar (which owns the actual subprocess
/// and the Lua filters).
pub struct PandocDriver {
    client: SidecarClient,
}

impl PandocDriver {
    pub fn new(client: SidecarClient) -> Self {
        Self { client }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn build(
        &self,
        units: Vec<PandocUnit>,
        metadata: BookMetadata,
        output_path: &str,
        output_format: &str,
        template: Option<String>,
        css: Option<String>,
    ) -> Result<PandocResult> {
        let params = PandocParams {
            units,
            metadata: serde_json::to_value(&metadata)?,
            output_path: output_path.to_string(),
            output_format: output_format.to_string(),
            template,
            css,
        };
        self.client.pandoc_build(params).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_quotes_risky_scalars() {
        assert_eq!(yaml_scalar("Plain"), "Plain");
        assert_eq!(yaml_scalar("a: b"), "\"a: b\"");
        assert_eq!(yaml_scalar(""), "\"\"");
        assert_eq!(yaml_scalar("- item"), "\"- item\"");
    }

    #[test]
    fn metadata_yaml_contains_expected_fields() {
        let mut metadata = BookMetadata::new("Il Nome della Rosa");
        metadata.author = Some("Umberto Eco".into());
        metadata.language = Some("it".into());
        metadata
            .extra
            .insert("subject".into(), Value::String("novel".into()));
        let yaml = metadata.to_yaml();
        assert!(yaml.contains("title: Il Nome della Rosa"));
        assert!(yaml.contains("author: Umberto Eco"));
        assert!(yaml.contains("language: it"));
        assert!(yaml.contains("subject: novel"));
    }
}
