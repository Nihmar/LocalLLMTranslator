//! Bilingual editor, proofreader and suggestion lifecycle (PLAN.md §11.4).
//!
//! The editor asks the `editor` role to compare source and translation and
//! returns JSON behind `prompts/editor.schema.json`; the proofreader asks the
//! `proofreader` role to polish the target blocks, separated by
//! [`BLOCK_SEPARATOR`]. Both persist `suggestion` rows. Accepting one rewrites the
//! block translation with the pass as its origin — after a placeholder guard —
//! and recomposes the chunk's `target_md`, so an export right after the review
//! sees the accepted text.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use minijinja::{context, Environment};
use serde::Deserialize;
use serde_json::Value;
use sqlx::SqlitePool;

use super::chat_call::{run_chat_call, run_structured_call, ChatCall};
use super::PipelineDeps;
use crate::db::models::{Block, BlockTranslation, Chunk, QaFinding, Suggestion};
use crate::db::{new_id, now, repo};
use crate::error::{AppError, Result};
use crate::llm::ResponseFormat;
use crate::util::{clamp_chars, sha256_hex_str};

pub const EDIT_ROLE: &str = "editor";
pub const EDIT_JOB: &str = "edit_chunk";
pub const PROOFREAD_ROLE: &str = "proofreader";
pub const PROOFREAD_JOB: &str = "proofread_chunk";

/// Line the proofreader prompt uses between blocks.
pub const BLOCK_SEPARATOR: &str = "<!-- block -->";

const EDIT_SCHEMA_NAME: &str = "editor";
const MAX_ISSUES: usize = 40;
const MAX_FIELD_CHARS: usize = 2000;
const MAX_REASON_CHARS: usize = 500;
const MAX_PROOFREAD_CHARS: usize = 24_000;
const EDIT_MAX_TOKENS: u32 = 1500;
const PROOFREAD_MAX_TOKENS: u32 = 4000;

/// Fallback templates, byte-identical to the repository prompt files (a unit
/// test asserts that) so a project snapshot from before M4 still runs.
pub const DEFAULT_EDITOR_SYSTEM_TEMPLATE: &str = r#"You are a bilingual revision editor for a {{ source_language }} → {{ target_language }} book translation.
You compare SOURCE and TRANSLATION and report only real defects: mistranslation, omission, addition,
terminology violation, register break, broken Markdown, broken or moved placeholder.
You do not rewrite for taste. You propose the smallest correction that fixes the defect.
Blocks are numbered `[0]`, `[1]`, ... in both texts; the `block_index` of an issue is that number.
Copy `quote` verbatim from the translation and make `suggested` the text that replaces it.
Report an issue only if you are confident; an empty issue list is a valid answer.
Dialogue punctuation (a dash or quotation marks) is a choice made for the whole book, never a defect.
Reply with JSON only."#;

pub const DEFAULT_EDITOR_USER_TEMPLATE: &str = r#"SOURCE ({{ source_language }}):
{{ source_text }}

TRANSLATION ({{ target_language }}):
{{ target_text }}

Reply with a single JSON object that validates against this schema:
{{ response_schema }}
"#;

pub const DEFAULT_EDITOR_SCHEMA: &str = r##"{"type":"object","properties":{"verdict":{"enum":["ok","needs_fix"]},"issues":{"type":"array","items":{"type":"object","properties":{"block_index":{"type":"integer"},"severity":{"enum":["critical","major","minor"]},"kind":{"enum":["meaning","omission","addition","terminology","register","markup","placeholder"]},"quote":{"type":"string"},"suggested":{"type":"string"},"reason":{"type":"string"}},"required":["block_index","kind","quote","suggested","reason"]}}},"required":["verdict","issues"]}"##;

pub const DEFAULT_PROOFREADER_SYSTEM_TEMPLATE: &str = r#"You are a monolingual proofreader for {{ target_language }}.
The text was translated from {{ source_language }} and reads slightly foreign.
Fix grammar, agreement, punctuation, calques, false friends and unnatural collocations.
Do NOT change meaning. Do NOT add or remove content. Do NOT touch placeholders ⟦n⟧.
Do NOT alter Markdown structure, code spans, URLs or table pipes.
Do NOT change how dialogue is punctuated (a dash or quotation marks): it is a choice made for the whole book.
The text arrives as blocks separated by a line containing only `<!-- block -->`.
Keep the same number of blocks, in the same order, with the same separators.
Output only the corrected text, with no commentary and no code fences."#;

pub const DEFAULT_PROOFREADER_USER_TEMPLATE: &str = r#"{{ text }}
"#;

/// SHA-256 of the system templates earlier releases shipped
/// (see [`crate::util::ensure_prompt_file`]).
const SHIPPED_EDITOR_SYSTEM_HASHES: &[&str] =
    &["e1b5271f0108c0f3ec1a8287144aa57a5bc528b220e66deb107c595c2334f54b"];
const SHIPPED_PROOFREADER_SYSTEM_HASHES: &[&str] =
    &["c4b6ace99c38f9e6e2c818528a791c7b87cf6282a54679012ce3d00f9ea85de2"];

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct EditedIssue {
    block_index: Option<i64>,
    severity: String,
    kind: String,
    quote: String,
    suggested: String,
    reason: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct RawEditor {
    #[serde(default)]
    issues: Vec<RawIssue>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct RawIssue {
    #[serde(default)]
    block_index: Option<i64>,
    #[serde(default)]
    severity: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    quote: String,
    #[serde(default)]
    suggested: String,
    #[serde(default)]
    reason: String,
}

fn normalize_severity(raw: &str) -> String {
    match raw.trim().to_lowercase().as_str() {
        "critical" => "critical",
        "major" => "major",
        _ => "minor",
    }
    .to_string()
}

/// Pull the JSON object out of a reply that may carry code fences or prose.
fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| &text[start..=end])
}

fn parse_editor_answer(text: &str) -> Result<Vec<EditedIssue>> {
    let json = extract_json_object(text)
        .ok_or_else(|| AppError::Invalid("the editor answer contains no JSON object".into()))?;
    let raw: RawEditor = serde_json::from_str(json).map_err(|error| {
        AppError::Invalid(format!("the editor answer is not valid JSON: {error}"))
    })?;

    let mut issues = Vec::new();
    for issue in raw.issues {
        let suggested = clamp_chars(&issue.suggested, MAX_FIELD_CHARS);
        if suggested.is_empty() {
            continue;
        }
        issues.push(EditedIssue {
            block_index: issue.block_index,
            severity: normalize_severity(&issue.severity),
            kind: clamp_chars(&issue.kind, 40),
            quote: clamp_chars(&issue.quote, MAX_FIELD_CHARS),
            suggested,
            reason: clamp_chars(&issue.reason, MAX_REASON_CHARS),
        });
        if issues.len() >= MAX_ISSUES {
            break;
        }
    }
    Ok(issues)
}

/// Number the blocks the way both prompts expect them, `[0]`, `[1]`, ...
fn numbered(blocks: &[&Block], texts: &[String]) -> String {
    blocks
        .iter()
        .zip(texts.iter())
        .enumerate()
        .map(|(index, (_, text))| format!("[{index}] {text}"))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn origin_rank(origin: &str) -> u8 {
    match origin {
        "user" => 3,
        "proofreader" => 2,
        "editor" => 1,
        _ => 0,
    }
}

/// The text currently in effect for a block: the most recently updated
/// translation, with the pass order as a tie-breaker.
fn preferred_translation<'a>(
    block_id: &str,
    translations: &'a [BlockTranslation],
) -> Option<&'a str> {
    translations
        .iter()
        .filter(|row| row.block_id == block_id && !row.text_md.trim().is_empty())
        .max_by_key(|row| (row.updated_at.clone(), origin_rank(&row.origin)))
        .map(|row| row.text_md.as_str())
}

/// Split the proofreader answer on the separator lines. A missing separator
/// yields one block, which the caller detects as a count mismatch.
fn split_blocks_text(text: &str) -> Vec<String> {
    let mut blocks: Vec<String> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in text.lines() {
        if line.trim() == BLOCK_SEPARATOR {
            blocks.push(current.join("\n").trim().to_string());
            current.clear();
        } else {
            current.push(line);
        }
    }
    blocks.push(current.join("\n").trim().to_string());
    blocks
}

/// Strip a single code fence wrapping the whole answer, if present.
fn strip_code_fence(text: &str) -> String {
    let trimmed = text.trim();
    if !trimmed.starts_with("```") || !trimmed.ends_with("```") || trimmed.len() < 6 {
        return text.to_string();
    }
    let inner = &trimmed[3..trimmed.len() - 3];
    inner
        .split_once('\n')
        .map(|(_, rest)| rest.trim().to_string())
        .unwrap_or_else(|| inner.trim().to_string())
}

// ---------------------------------------------------------------------------
// Prompt files
// ---------------------------------------------------------------------------

/// Write the review prompt files into a project snapshot, never overwriting an
/// edited one.
pub async fn ensure_prompt_files(dir: &Path) -> Result<()> {
    tokio::fs::create_dir_all(dir).await?;
    for (name, content, shipped) in [
        (
            "editor.system.md",
            DEFAULT_EDITOR_SYSTEM_TEMPLATE,
            SHIPPED_EDITOR_SYSTEM_HASHES,
        ),
        ("editor.user.md", DEFAULT_EDITOR_USER_TEMPLATE, &[]),
        ("editor.schema.json", DEFAULT_EDITOR_SCHEMA, &[]),
        (
            "proofreader.system.md",
            DEFAULT_PROOFREADER_SYSTEM_TEMPLATE,
            SHIPPED_PROOFREADER_SYSTEM_HASHES,
        ),
        (
            "proofreader.user.md",
            DEFAULT_PROOFREADER_USER_TEMPLATE,
            &[],
        ),
    ] {
        crate::util::ensure_prompt_file(&dir.join(name), content, shipped).await?;
    }
    Ok(())
}

fn read_first(dir: &Path, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| std::fs::read_to_string(dir.join(name)).ok())
}

fn load_editor(dir: &Path) -> (String, String, Value) {
    let system = read_first(dir, &["editor.system.md"])
        .unwrap_or_else(|| DEFAULT_EDITOR_SYSTEM_TEMPLATE.to_string());
    let user = read_first(dir, &["editor.user.md"])
        .unwrap_or_else(|| DEFAULT_EDITOR_USER_TEMPLATE.to_string());
    let schema = read_first(dir, &["editor.schema.json"])
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| {
            serde_json::from_str(DEFAULT_EDITOR_SCHEMA).expect("embedded editor schema is valid")
        });
    (system, user, schema)
}

fn load_proofreader(dir: &Path) -> (String, String) {
    let system = read_first(dir, &["proofreader.system.md"])
        .unwrap_or_else(|| DEFAULT_PROOFREADER_SYSTEM_TEMPLATE.to_string());
    let user = read_first(dir, &["proofreader.user.md"])
        .unwrap_or_else(|| DEFAULT_PROOFREADER_USER_TEMPLATE.to_string());
    (system, user)
}

// ---------------------------------------------------------------------------
// Running the passes
// ---------------------------------------------------------------------------

struct Preparation {
    project_id: String,
    blocks: Vec<Block>,
    texts: Vec<String>,
}

async fn prepare(pool: &SqlitePool, chunk_id: &str) -> Result<(Chunk, Preparation)> {
    let chunk = repo::get_chunk(pool, chunk_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("chunk {chunk_id}")))?;
    let project_id: String = sqlx::query_scalar("SELECT project_id FROM document WHERE id = ?1")
        .bind(&chunk.document_id)
        .fetch_one(pool)
        .await?;
    let block_ids: Vec<String> = serde_json::from_str(&chunk.block_ids_json).unwrap_or_default();
    let document_blocks = repo::list_blocks(pool, &chunk.document_id).await?;
    let chapter_blocks: Vec<Block> = block_ids
        .iter()
        .filter_map(|id| {
            document_blocks
                .iter()
                .find(|block| &block.id == id)
                .cloned()
        })
        .collect();
    let translations = repo::list_block_translations(pool, chunk_id).await?;
    let blocks: Vec<Block> = chapter_blocks
        .into_iter()
        .filter(|block| block.translatable)
        .collect();
    if blocks.is_empty() {
        return Err(AppError::Invalid(format!(
            "chunk {chunk_id} has no translatable block"
        )));
    }
    let texts: Vec<String> = blocks
        .iter()
        .map(|block| {
            preferred_translation(&block.id, &translations)
                .map(str::to_string)
                .unwrap_or_else(|| block.source_md.clone())
        })
        .collect();
    Ok((
        chunk,
        Preparation {
            project_id,
            blocks,
            texts,
        },
    ))
}

/// Bilingual editor pass: one structured call, one suggestion per real defect.
pub async fn run_edit_chunk(
    deps: &PipelineDeps,
    job_id: Option<&str>,
    chunk_id: &str,
) -> Result<usize> {
    let pool = &deps.pool;
    let (chunk, prep) = prepare(pool, chunk_id).await?;
    let project = repo::get_project(pool, &prep.project_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("project {}", prep.project_id)))?;

    let binding = repo::role_binding_for(pool, EDIT_ROLE)
        .await?
        .ok_or_else(|| AppError::Invalid("no role_binding configured for 'editor'".into()))?;
    let endpoint = repo::get_endpoint(pool, &binding.endpoint_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("endpoint {}", binding.endpoint_id)))?;

    let prompts_dir = deps.prompts_dir(&prep.project_id);
    ensure_prompt_files(&prompts_dir).await?;
    let (system_template, user_template, schema) = load_editor(&prompts_dir);

    let block_refs: Vec<&Block> = prep.blocks.iter().collect();
    let source_texts: Vec<String> = prep
        .blocks
        .iter()
        .map(|block| block.source_md.clone())
        .collect();
    let source_text = numbered(&block_refs, &source_texts);
    let target_text = numbered(&block_refs, &prep.texts);
    let schema_text = serde_json::to_string_pretty(&schema)?;

    let env = Environment::new();
    let system = env.render_str(
        &system_template,
        context! {
            source_language => project.source_lang.clone().unwrap_or_default(),
            target_language => &project.target_lang,
        },
    )?;
    let user = env.render_str(
        &user_template,
        context! {
            source_language => project.source_lang.clone().unwrap_or_default(),
            target_language => &project.target_lang,
            source_text => &source_text,
            target_text => &target_text,
            response_schema => &schema_text,
        },
    )?;
    let prompt_hash = sha256_hex_str(&format!("{system}\n\u{0}\n{user}"));

    let (_, issues) = run_structured_call(
        deps,
        &ChatCall {
            job_id,
            chunk_id: Some(&chunk.id),
            role: EDIT_ROLE,
            endpoint_id: &endpoint.id,
            base_url: &endpoint.base_url,
            model: &binding.model,
            params_json: &binding.params_json,
            prompt_hash: &prompt_hash,
            system: &system,
            user: &user,
            response_format: Some(ResponseFormat::json_schema(EDIT_SCHEMA_NAME, schema)),
            seed: crate::pipeline::translate::derive_seed(&chunk.id, EDIT_ROLE),
            default_max_tokens: Some(EDIT_MAX_TOKENS),
        },
        parse_editor_answer,
    )
    .await?;

    repo::supersede_suggestions(pool, chunk_id, EDIT_ROLE).await?;
    let mut created = 0;
    for issue in issues {
        let block = issue
            .block_index
            .and_then(|index| usize::try_from(index).ok())
            .and_then(|index| prep.blocks.get(index));
        let Some(block) = block else {
            continue;
        };
        let current = preferred_translation(
            &block.id,
            &repo::list_block_translations(pool, chunk_id).await?,
        )
        .map(str::to_string)
        .unwrap_or_else(|| block.source_md.clone());
        repo::insert_suggestion(
            pool,
            &Suggestion {
                id: new_id(),
                chunk_id: chunk_id.to_string(),
                pass: EDIT_ROLE.to_string(),
                block_id: Some(block.id.clone()),
                field: Some("text".to_string()),
                original: Some(current),
                proposed: Some(issue.suggested),
                reason: (!issue.reason.is_empty()).then_some(issue.reason),
                severity: Some(issue.severity),
                quote: (!issue.quote.is_empty()).then_some(issue.quote),
                status: "pending".to_string(),
                created_at: now(),
                decided_at: None,
            },
        )
        .await?;
        created += 1;
    }
    Ok(created)
}

/// Monolingual proofreader pass: one block-per-suggestion call.
pub async fn run_proofread_chunk(
    deps: &PipelineDeps,
    job_id: Option<&str>,
    chunk_id: &str,
) -> Result<usize> {
    let pool = &deps.pool;
    let (chunk, prep) = prepare(pool, chunk_id).await?;
    let project = repo::get_project(pool, &prep.project_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("project {}", prep.project_id)))?;

    let binding = repo::role_binding_for(pool, PROOFREAD_ROLE)
        .await?
        .ok_or_else(|| AppError::Invalid("no role_binding configured for 'proofreader'".into()))?;
    let endpoint = repo::get_endpoint(pool, &binding.endpoint_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("endpoint {}", binding.endpoint_id)))?;

    let separator = format!("\n\n{BLOCK_SEPARATOR}\n\n");
    let target_text = prep.texts.join(&separator);
    if target_text.chars().count() > MAX_PROOFREAD_CHARS {
        return Err(AppError::Invalid(format!(
            "chunk {chunk_id} is too large for the proofreader pass"
        )));
    }

    let prompts_dir = deps.prompts_dir(&prep.project_id);
    ensure_prompt_files(&prompts_dir).await?;
    let (system_template, user_template) = load_proofreader(&prompts_dir);

    let env = Environment::new();
    let system = env.render_str(
        &system_template,
        context! {
            source_language => project.source_lang.clone().unwrap_or_default(),
            target_language => &project.target_lang,
        },
    )?;
    let user = env.render_str(&user_template, context! { text => &target_text })?;
    let prompt_hash = sha256_hex_str(&format!("{system}\n\u{0}\n{user}"));

    let response = run_chat_call(
        deps,
        &ChatCall {
            job_id,
            chunk_id: Some(&chunk.id),
            role: PROOFREAD_ROLE,
            endpoint_id: &endpoint.id,
            base_url: &endpoint.base_url,
            model: &binding.model,
            params_json: &binding.params_json,
            prompt_hash: &prompt_hash,
            system: &system,
            user: &user,
            response_format: None,
            seed: crate::pipeline::translate::derive_seed(&chunk.id, PROOFREAD_ROLE),
            default_max_tokens: Some(PROOFREAD_MAX_TOKENS),
        },
    )
    .await?;
    let corrected = split_blocks_text(&strip_code_fence(&response));

    if corrected.len() != prep.blocks.len() {
        repo::insert_qa_finding(
            pool,
            &QaFinding {
                id: new_id(),
                project_id: prep.project_id.clone(),
                chunk_id: Some(chunk.id.clone()),
                block_id: None,
                kind: "markdown_malformed".to_string(),
                severity: "minor".to_string(),
                details_json: serde_json::json!({
                    "reason": "proofreader_block_count",
                    "expected": prep.blocks.len(),
                    "got": corrected.len(),
                })
                .to_string(),
                status: "open".to_string(),
                created_at: now(),
            },
        )
        .await?;
        return Ok(0);
    }

    repo::supersede_suggestions(pool, chunk_id, PROOFREAD_ROLE).await?;
    let mut created = 0;
    for (block, text) in prep.blocks.iter().zip(corrected.iter()) {
        let current = preferred_translation(
            &block.id,
            &repo::list_block_translations(pool, chunk_id).await?,
        )
        .map(str::to_string)
        .unwrap_or_else(|| block.source_md.clone());
        if text.trim().is_empty() || text.trim() == current.trim() {
            continue;
        }
        repo::insert_suggestion(
            pool,
            &Suggestion {
                id: new_id(),
                chunk_id: chunk_id.to_string(),
                pass: PROOFREAD_ROLE.to_string(),
                block_id: Some(block.id.clone()),
                field: Some("text".to_string()),
                original: Some(current),
                proposed: Some(text.clone()),
                reason: None,
                severity: None,
                quote: None,
                status: "pending".to_string(),
                created_at: now(),
                decided_at: None,
            },
        )
        .await?;
        created += 1;
    }
    Ok(created)
}

// ---------------------------------------------------------------------------
// Accept / reject
// ---------------------------------------------------------------------------

/// Select the eligible chunks and enqueue the requested passes. Returns the new
/// job ids; an equivalent pending job suppresses a duplicate.
pub async fn enqueue_review_jobs(
    pool: &SqlitePool,
    project_id: &str,
    chunk_ids: Option<&[String]>,
    chapter_id: Option<&str>,
    pass: &str,
    with_qa: bool,
) -> Result<Vec<String>> {
    let chunks = repo::list_chunks_by_project(pool, project_id, None).await?;
    let wanted: Option<HashSet<&str>> =
        chunk_ids.map(|ids| ids.iter().map(String::as_str).collect());
    let mut jobs = Vec::new();

    for chunk in &chunks {
        if !matches!(chunk.status.as_str(), "done" | "needs_review") {
            continue;
        }
        if chunk
            .target_md
            .as_deref()
            .is_none_or(|text| text.trim().is_empty())
        {
            continue;
        }
        if let Some(chapter) = chapter_id {
            if chunk.chapter_id.as_deref() != Some(chapter) {
                continue;
            }
        }
        if let Some(wanted) = &wanted {
            if !wanted.contains(chunk.id.as_str()) {
                continue;
            }
        }

        let payload = serde_json::json!({ "chunk_id": chunk.id });
        let mut kinds: Vec<&str> = Vec::new();
        if pass != "proofreader" {
            kinds.push(EDIT_JOB);
        }
        if pass != "editor" {
            kinds.push(PROOFREAD_JOB);
        }
        if with_qa {
            kinds.push(super::qa::JOB_KIND);
        }
        for kind in kinds {
            if crate::scheduler::queue::has_pending(pool, project_id, kind, &payload).await? {
                continue;
            }
            let job = crate::scheduler::NewJob::new(project_id, kind, payload.clone())
                .with_priority(60 + chunk.order_index);
            jobs.push(crate::scheduler::queue::enqueue(pool, &job).await?);
        }
    }
    Ok(jobs)
}

/// Re-write a chunk's `target_md` from its current block translations, so the
/// exporter sees the accepted text.
pub async fn recompose_chunk(pool: &SqlitePool, chunk_id: &str) -> Result<()> {
    let chunk = repo::get_chunk(pool, chunk_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("chunk {chunk_id}")))?;
    let block_ids: Vec<String> = serde_json::from_str(&chunk.block_ids_json).unwrap_or_default();
    let blocks = repo::list_blocks(pool, &chunk.document_id).await?;
    let translations = repo::list_block_translations(pool, chunk_id).await?;

    let mut parts = Vec::with_capacity(block_ids.len());
    for id in &block_ids {
        let Some(block) = blocks.iter().find(|block| &block.id == id) else {
            continue;
        };
        let text = if block.translatable {
            preferred_translation(&block.id, &translations)
                .map(str::to_string)
                .unwrap_or_else(|| block.source_md.clone())
        } else {
            block.source_md.clone()
        };
        parts.push(text);
    }
    repo::update_chunk_target(pool, chunk_id, &parts.join("\n\n")).await
}

/// Block markers the extractor escapes at the start of a line (`\-` for a dialogue
/// dash, `\#`, `\>`, …) so a paragraph does not turn into a list, a heading or a
/// quote. Ordered-list markers (`1\.`) are handled separately.
const LINE_MARKERS: &str = "-+*#>|";

/// Whether a placeholder literal is a line-start escape (`\-`, `12\.`): a markdown
/// artefact, not content, so the guard does not count it (see
/// [`restore_line_escapes`]).
fn is_line_escape(literal: &str) -> bool {
    let Some(rest) = literal.strip_suffix(['.', ')']) else {
        let mut chars = literal.chars();
        return chars.next() == Some('\\')
            && chars.next().is_some_and(|c| LINE_MARKERS.contains(c))
            && chars.next().is_none();
    };
    rest.strip_suffix('\\')
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

/// The escaped marker a line opens with (`\- Sit` → `-`, `3\. Then` → `.`).
fn escaped_marker(line: &str) -> Option<char> {
    if let Some(rest) = line.strip_prefix('\\') {
        return rest.chars().next().filter(|c| LINE_MARKERS.contains(*c));
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    let rest = line[digits..].strip_prefix('\\')?;
    (digits > 0)
        .then(|| rest.chars().next())
        .flatten()
        .filter(|c| matches!(c, '.' | ')'))
}

/// The line with its block marker escaped, when it opens with an unescaped one.
fn escape_line(line: &str) -> Option<(char, String)> {
    let mut chars = line.chars();
    let first = chars.next()?;
    if LINE_MARKERS.contains(first) {
        // `-`, `+` and `*` only start a list when a space follows.
        let needs_space = matches!(first, '-' | '+' | '*');
        if needs_space && chars.next() != Some(' ') {
            return None;
        }
        return Some((first, format!("\\{line}")));
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let marker = line[digits..]
        .chars()
        .next()
        .filter(|c| matches!(c, '.' | ')'))?;
    line[digits + 1..]
        .starts_with(' ')
        .then(|| (marker, format!("{}\\{}", &line[..digits], &line[digits..])))
}

/// Re-add the line-start escapes a correction lost.
///
/// A correction may drop an escape on purpose — `\- But…` → `"But…"` no longer opens
/// with a marker, so there is nothing to escape — or lose only the backslash, which
/// would turn a dialogue paragraph into a list. Only the second case is repaired, and
/// only for markers the current text escapes, so a correction never gains an escape
/// the extractor did not put there.
fn restore_line_escapes(current: &str, proposed: &str) -> String {
    let escaped: Vec<char> = current.lines().filter_map(escaped_marker).collect();
    proposed
        .split('\n')
        .map(|line| match escape_line(line) {
            Some((marker, fixed)) if escaped.contains(&marker) => fixed,
            _ => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A change must not lose or duplicate a placeholder literal of its block.
/// Best-effort: when the sidecar is unreachable the guard is skipped rather than
/// blocking the review.
async fn guard_placeholders(
    deps: &PipelineDeps,
    chunk_id: &str,
    old_text: &str,
    new_text: &str,
) -> Result<()> {
    // Never start the sidecar just for this best-effort check.
    if !deps.sidecar.is_running() {
        return Ok(());
    }
    let Ok(Some(chunk)) = repo::get_chunk(&deps.pool, chunk_id).await else {
        return Ok(());
    };
    let block_ids: Vec<String> = serde_json::from_str(&chunk.block_ids_json).unwrap_or_default();
    let Ok(prepared) = deps
        .sidecar
        .prepare_text(Some(&block_ids), &chunk.source_md)
        .await
    else {
        return Ok(());
    };

    let mut literals: Vec<&str> = prepared
        .placeholders
        .iter()
        .map(|(_, literal)| literal.as_str())
        .collect();
    literals.sort_unstable();
    literals.dedup();
    for literal in literals {
        // Line-start escapes are repaired by `restore_line_escapes`, not counted.
        if literal.is_empty() || is_line_escape(literal) {
            continue;
        }
        let before = old_text.matches(literal).count();
        let after = new_text.matches(literal).count();
        if before != after {
            return Err(AppError::Invalid(format!(
                "the change would alter the placeholder {literal:?} ({before} → {after})"
            )));
        }
    }
    Ok(())
}

/// Apply the accepted correction to the block translation and recompose.
pub async fn accept_suggestion(deps: &PipelineDeps, id: &str) -> Result<Suggestion> {
    let pool = &deps.pool;
    let suggestion = repo::get_suggestion(pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("suggestion {id}")))?;
    if suggestion.status != "pending" {
        return Ok(suggestion);
    }
    let block_id = suggestion
        .block_id
        .clone()
        .ok_or_else(|| AppError::Invalid(format!("suggestion {id} has no block")))?;
    let block = repo::get_block(pool, &block_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("block {block_id}")))?;
    let translations = repo::list_block_translations(pool, &suggestion.chunk_id).await?;
    let current = preferred_translation(&block_id, &translations)
        .map(str::to_string)
        .unwrap_or_else(|| block.source_md.clone());

    let proposed = suggestion
        .proposed
        .clone()
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| AppError::Invalid(format!("suggestion {id} has no proposed text")))?;
    let new_text = if suggestion.pass == PROOFREAD_ROLE {
        proposed
    } else {
        match suggestion
            .quote
            .as_deref()
            .filter(|quote| !quote.is_empty())
        {
            Some(quote) if current.contains(quote) => current.replacen(quote, &proposed, 1),
            // No usable quote: the proposal is the corrected block.
            _ => proposed,
        }
    };

    let new_text = restore_line_escapes(&current, &new_text);
    guard_placeholders(deps, &suggestion.chunk_id, &current, &new_text).await?;

    repo::upsert_block_translation(
        pool,
        &BlockTranslation {
            block_id: block_id.clone(),
            chunk_id: suggestion.chunk_id.clone(),
            text_md: new_text,
            placeholders_ok: true,
            origin: suggestion.pass.clone(),
            edited_by_user: false,
            updated_at: now(),
        },
    )
    .await?;
    repo::supersede_suggestions_for_block(pool, &suggestion.chunk_id, &suggestion.pass, &block_id)
        .await?;
    repo::set_suggestion_status(pool, id, "accepted").await?;
    recompose_chunk(pool, &suggestion.chunk_id).await?;

    repo::get_suggestion(pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("suggestion {id}")))
}

pub async fn reject_suggestion(pool: &SqlitePool, id: &str) -> Result<Suggestion> {
    let suggestion = repo::get_suggestion(pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("suggestion {id}")))?;
    if suggestion.status == "pending" {
        repo::set_suggestion_status(pool, id, "rejected").await?;
    }
    repo::get_suggestion(pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("suggestion {id}")))
}

/// Block texts in effect, for a snapshot or a test.
pub async fn current_texts(pool: &SqlitePool, chunk_id: &str) -> Result<HashMap<String, String>> {
    let chunk = repo::get_chunk(pool, chunk_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("chunk {chunk_id}")))?;
    let block_ids: Vec<String> = serde_json::from_str(&chunk.block_ids_json).unwrap_or_default();
    let blocks = repo::list_blocks(pool, &chunk.document_id).await?;
    let translations = repo::list_block_translations(pool, chunk_id).await?;
    let mut texts = HashMap::new();
    for id in block_ids {
        let Some(block) = blocks.iter().find(|block| block.id == id) else {
            continue;
        };
        let text = preferred_translation(&block.id, &translations)
            .map(str::to_string)
            .unwrap_or_else(|| block.source_md.clone());
        texts.insert(block.id.clone(), text);
    }
    Ok(texts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_escapes_are_recognised() {
        for literal in ["\\-", "\\#", "\\>", "\\|", "1\\.", "12\\)"] {
            assert!(is_line_escape(literal), "{literal} is a line escape");
        }
        for literal in ["\\", "-", "\\a", "\\.", "a\\.", "1.", "⟦1⟧", "**bold**"] {
            assert!(!is_line_escape(literal), "{literal} is not a line escape");
        }
    }

    #[test]
    fn a_dialogue_line_may_drop_its_escape_for_quotes() {
        let current = "\\- But why don't you tell them? he had wondered.";
        let proposed = "\"But why don't you tell them?\" he had wondered.";
        assert_eq!(restore_line_escapes(current, proposed), proposed);
    }

    #[test]
    fn a_lost_backslash_is_put_back() {
        let current = "\\- Sit down.\n\\- Why?\n3\\. Then";
        let proposed = "- Sit down, now.\n- Why?\n3. Then";
        assert_eq!(
            restore_line_escapes(current, proposed),
            "\\- Sit down, now.\n\\- Why?\n3\\. Then"
        );
    }

    #[test]
    fn no_escape_is_added_where_the_text_had_none() {
        let current = "A plain paragraph.";
        let proposed = "- now a list item\n# and a heading";
        assert_eq!(restore_line_escapes(current, proposed), proposed);
        // `-` without a space is a word, never a list marker.
        assert_eq!(
            restore_line_escapes("\\- x", "-ish is fine"),
            "-ish is fine"
        );
    }

    fn block(id: &str, source: &str) -> Block {
        Block {
            id: id.to_string(),
            document_id: "d".to_string(),
            chapter_id: None,
            order_index: 0,
            kind: "para".to_string(),
            level: 0,
            source_md: source.to_string(),
            source_text: source.to_string(),
            translatable: true,
            attrs_json: "{}".to_string(),
            content_hash: format!("h-{id}"),
        }
    }

    fn translation(block_id: &str, origin: &str, text: &str, updated_at: &str) -> BlockTranslation {
        BlockTranslation {
            block_id: block_id.to_string(),
            chunk_id: "c1".to_string(),
            text_md: text.to_string(),
            placeholders_ok: true,
            origin: origin.to_string(),
            edited_by_user: false,
            updated_at: updated_at.to_string(),
        }
    }

    #[test]
    fn numbered_prefixes_every_block() {
        let first = block("b1", "First");
        let second = block("b2", "Second");
        let blocks = [&first, &second];
        let texts = ["Uno".to_string(), "Due".to_string()];
        assert_eq!(numbered(&blocks, &texts), "[0] Uno\n\n[1] Due");
    }

    #[test]
    fn preferred_translation_takes_the_most_recent_pass() {
        let rows = [
            translation("b1", "translator", "vecchio", "2026-01-01T00:00:00Z"),
            translation("b1", "proofreader", "recente", "2026-01-02T00:00:00Z"),
            translation("b2", "translator", "solo", "2026-01-03T00:00:00Z"),
        ];
        assert_eq!(preferred_translation("b1", &rows), Some("recente"));
        assert_eq!(preferred_translation("b2", &rows), Some("solo"));
        // The pass order breaks a tie.
        let tied = [
            translation("b3", "translator", "base", "2026-01-04T00:00:00Z"),
            translation("b3", "user", "mano", "2026-01-04T00:00:00Z"),
        ];
        assert_eq!(preferred_translation("b3", &tied), Some("mano"));
    }

    #[test]
    fn editor_answer_is_clamped_and_unusable_issues_dropped() {
        let answer = r#"{"verdict":"needs_fix","issues":[
            {"block_index":0,"severity":"critical","kind":"meaning","quote":"q","suggested":"fixed","reason":"r"},
            {"block_index":1,"severity":"nonsense","kind":"register","quote":"q","suggested":"","reason":"r"},
            {"block_index":2,"severity":"minor","kind":"register","quote":"","suggested":"ok","reason":"r"}
        ]}"#;
        let issues = parse_editor_answer(answer).expect("parse");
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].severity, "critical");
        assert_eq!(issues[0].suggested, "fixed");
        assert_eq!(issues[0].block_index, Some(0));
        assert_eq!(issues[1].severity, "minor");
        assert_eq!(issues[1].quote, "");
    }

    #[test]
    fn proofreader_answer_splits_on_the_separator() {
        let text = "Primo blocco.\n\n<!-- block -->\n\nSecondo\nblocco.";
        let blocks = split_blocks_text(text);
        assert_eq!(blocks, vec!["Primo blocco.", "Secondo\nblocco."]);
        // A missing separator is one block, which the caller detects.
        assert_eq!(split_blocks_text("nessun separatore").len(), 1);
    }

    #[test]
    fn code_fences_are_stripped() {
        assert_eq!(strip_code_fence("```\ncorretto\n```"), "corretto");
        assert_eq!(strip_code_fence("```text\ncorretto\n```"), "corretto");
        assert_eq!(strip_code_fence("lasciato"), "lasciato");
    }

    #[test]
    fn embedded_prompts_match_the_repository_files() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let editor = std::fs::read_to_string(root.join("prompts/editor.md"))
            .expect("read prompts/editor.md");
        let (system, user) = editor
            .split_once("-->\n")
            .expect("editor header")
            .1
            .split_once("\n---USER---\n")
            .expect("editor marker");
        assert_eq!(system, DEFAULT_EDITOR_SYSTEM_TEMPLATE);
        assert_eq!(user, DEFAULT_EDITOR_USER_TEMPLATE);

        let proofreader = std::fs::read_to_string(root.join("prompts/proofreader.md"))
            .expect("read prompts/proofreader.md");
        let (system, user) = proofreader
            .split_once("-->\n")
            .expect("proofreader header")
            .1
            .split_once("\n---USER---\n")
            .expect("proofreader marker");
        assert_eq!(system, DEFAULT_PROOFREADER_SYSTEM_TEMPLATE);
        assert_eq!(user, DEFAULT_PROOFREADER_USER_TEMPLATE);

        let file: Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("prompts/editor.schema.json"))
                .expect("read prompts/editor.schema.json"),
        )
        .expect("editor schema is valid JSON");
        let embedded: Value =
            serde_json::from_str(DEFAULT_EDITOR_SCHEMA).expect("embedded schema is valid JSON");
        assert_eq!(file, embedded);
    }
}
