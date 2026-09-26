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

    /// Reserved keys owned by the typed fields above; a source document never
    /// overrides them, because a duplicate YAML key would shadow the real value.
    const RESERVED_KEYS: [&'static str; 3] = ["title", "author", "language"];

    /// Fold the source document's front matter into [`BookMetadata::extra`].
    ///
    /// The typed fields (`title`, `author`, `language`) and any key the caller
    /// has already set explicitly win: only new, non-null keys are added, so the
    /// project row is never silently overridden by the document.
    pub fn merge_front_matter(&mut self, front_matter: &serde_json::Map<String, Value>) {
        for (key, value) in front_matter {
            if Self::RESERVED_KEYS.contains(&key.as_str()) || value.is_null() {
                continue;
            }
            if self.extra.contains_key(key) {
                continue;
            }
            self.extra.insert(key.clone(), value.clone());
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

/// Environment variable overriding the pandoc assets directory (templates,
/// filters, styles).
pub const PANDOC_DIR_ENV: &str = "LLMTRANSLATOR_PANDOC_DIR";

/// Resolve the directory holding the user-editable pandoc assets, in the same
/// order as the sidecar binary: the environment override, the bundled resource
/// directory, then the repository's `pandoc/` (development and tests).
pub fn resolve_assets_dir(resource_dir: Option<&Path>) -> Option<PathBuf> {
    if let Ok(override_dir) = std::env::var(PANDOC_DIR_ENV) {
        let path = PathBuf::from(override_dir);
        if path.is_dir() {
            return Some(path);
        }
    }
    if let Some(resource_dir) = resource_dir {
        let path = resource_dir.join("pandoc");
        if path.is_dir() {
            return Some(path);
        }
    }
    // Compile-time repository path: present in development and tests, absent in
    // a bundled install (where the resource directory takes over).
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("pandoc");
    repo.is_dir().then_some(repo)
}

/// A resolved file inside the assets directory, if it exists.
pub fn assets_file(assets: Option<&Path>, subdir: &str, name: &str) -> Option<String> {
    let path = assets?.join(subdir).join(name);
    path.is_file().then(|| path.to_string_lossy().to_string())
}

/// Everything one pandoc invocation needs.
#[derive(Debug, Clone, Default)]
pub struct PandocBuild {
    pub units: Vec<PandocUnit>,
    pub metadata: BookMetadata,
    pub output_path: String,
    pub output_format: String,
    pub template: Option<String>,
    pub css: Option<String>,
    pub resource_path: Vec<String>,
    pub toc: bool,
    pub lua_filters: Vec<String>,
    pub top_level_division: Option<String>,
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

    pub async fn build(&self, build: PandocBuild) -> Result<PandocResult> {
        let params = PandocParams {
            units: build.units,
            metadata: serde_json::to_value(&build.metadata)?,
            output_path: build.output_path,
            output_format: build.output_format,
            template: build.template,
            css: build.css,
            resource_path: build.resource_path,
            toc: build.toc,
            lua_filters: build.lua_filters,
            top_level_division: build.top_level_division,
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

    #[test]
    fn merge_front_matter_adds_document_keys_without_overriding_explicit_ones() {
        let mut metadata = BookMetadata::new("The Lantern Keeper");
        metadata.author = Some("Fixture Author".into());
        metadata.language = Some("it".into());
        metadata
            .extra
            .insert("publisher".into(), Value::String("Explicit Press".into()));

        let front_matter = serde_json::json!({
            "title": "A Different Title",
            "author": "A Different Author",
            "language": "en",
            "publisher": "Source Press",
            "date": "2020",
            "identifier": "urn:uuid:1",
            "subject": "novel",
            "empty": Value::Null,
        });
        let map = match front_matter {
            Value::Object(map) => map,
            other => panic!("front matter must be an object, got {other:?}"),
        };
        metadata.merge_front_matter(&map);

        // Reserved keys and an already-set extra keep their value.
        assert_eq!(metadata.title, "The Lantern Keeper");
        assert_eq!(metadata.author.as_deref(), Some("Fixture Author"));
        assert_eq!(metadata.language.as_deref(), Some("it"));
        assert_eq!(
            metadata.extra.get("publisher"),
            Some(&Value::String("Explicit Press".into()))
        );
        // New keys, including the source document's own, reach the output.
        assert_eq!(
            metadata.extra.get("date"),
            Some(&Value::String("2020".into()))
        );
        assert_eq!(
            metadata.extra.get("identifier"),
            Some(&Value::String("urn:uuid:1".into()))
        );
        assert_eq!(
            metadata.extra.get("subject"),
            Some(&Value::String("novel".into()))
        );
        // A null value is never carried into metadata.yaml.
        assert!(!metadata.extra.contains_key("empty"));

        let yaml = metadata.to_yaml();
        assert!(yaml.contains("title: The Lantern Keeper"));
        assert_eq!(yaml.matches("publisher:").count(), 1);
        assert!(yaml.contains("date: 2020"));
        assert!(
            yaml.contains("identifier: urn:uuid:1") || yaml.contains("identifier: \"urn:uuid:1\"")
        );
    }
}
