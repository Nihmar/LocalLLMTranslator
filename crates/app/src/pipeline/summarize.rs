//! Rolling chapter memory (PLAN.md section 8): chapter summaries, candidate
//! glossary terms and style-note candidates.
//!
//! A `summarize` job runs on the `orchestrator` role after every
//! [`ROLLING_EVERY`] completed chunks of a chapter and once when the chapter has
//! no unfinished chunk left. A rolling run writes
//! `project_memory['rolling_summary:<chapter_id>']`; the final run writes `chapter.summary`
//! (what the context assembler reads for the next chapters) and clears the rolling one
//! for that chapter. Nothing is imposed: candidate terms keep `status='candidate'`
//! and style notes wait for the user, so the confirmed book profile stays the
//! one authoritative head of the prompt.

use std::collections::BTreeSet;
use std::path::Path;

use minijinja::{context, Environment};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::SqlitePool;

use super::chat_call::{run_structured_call, ChatCall};
use super::PipelineDeps;
use crate::db::models::{Chunk, Project};
use crate::db::repo;
use crate::error::{AppError, Result};
use crate::util::{clamp_chars, clamp_list, clamp_words, sha256_hex_str};

/// The summarizer prompt runs on the orchestrator role (`prompts/summarizer.md`).
pub const ROLE: &str = "orchestrator";
pub const JOB_KIND: &str = "summarize";
/// A rolling update every this many completed chunks of a chapter.
pub const ROLLING_EVERY: i64 = 5;
/// Prefix of the per-chapter rolling summary key. The value is stored per chapter because
/// the translator injects it as "the current chapter so far": a project-wide key would let
/// a concurrent summary of another chapter leak into the prompt.
pub const ROLLING_SUMMARY_PREFIX: &str = "rolling_summary:";

/// Key of the rolling summary of one chapter. Chunks outside any chapter share a sentinel.
pub fn rolling_summary_key(chapter_id: Option<&str>) -> String {
    format!(
        "{ROLLING_SUMMARY_PREFIX}{}",
        chapter_id.unwrap_or(NO_CHAPTER_KEY)
    )
}

/// Chapter id used by chunks that sit outside any chapter (the preamble).
const NO_CHAPTER_KEY: &str = "__none__";
/// Project memory key holding style-note candidates (a JSON array of strings).
pub const STYLE_NOTES_KEY: &str = "style_notes";
/// `response_format.json_schema.name` the model sees.
const SCHEMA_NAME: &str = "chapter_summary";

const MAX_SUMMARY_WORDS: usize = 200;
const MAX_TERMS: usize = 8;
const MAX_TERM_CHARS: usize = 120;
const MAX_NOTE_CHARS: usize = 200;
const MAX_STYLE_NOTES: usize = 8;
const MAX_STYLE_NOTE_CHARS: usize = 240;
const MAX_STORED_STYLE_NOTES: usize = 16;
/// Shared by the source and the translation excerpt, half each.
const MAX_EXCERPT_CHARS: usize = 12_000;
/// Below this much source text a chapter is a title page or a heading: there is
/// nothing to summarise, and the model's notes about "no narrative text" are noise.
const MIN_SOURCE_CHARS: usize = 300;
const DEFAULT_MAX_TOKENS: u32 = 900;

/// Fallback templates, byte-identical to `prompts/summarizer.md` and
/// `prompts/summarizer.schema.json` (a unit test asserts that) so a project
/// snapshot from before M3 still runs.
pub const DEFAULT_SUMMARIZER_SYSTEM_TEMPLATE: &str = r#"You maintain the memory of a translation project ({{ source_language }} → {{ target_language }}).
You receive the same chapter twice: the SOURCE text and its TRANSLATION. Produce JSON only:
{"summary": "3-5 sentences in {{ target_language }}",
 "new_terms": [{"source":"","target":"","kind":"term|proper_noun|do_not_translate","note":""}],
 "style_notes": ["short observations about register, recurring constructions, forms of address"]}
In new_terms, "source" is copied exactly as the SOURCE text writes it and "target" is how the TRANSLATION renders it.
Output at most 8 new_terms, only terms that recur or matter."#;

pub const DEFAULT_SUMMARIZER_USER_TEMPLATE: &str = r#"CHAPTER: {{ chapter_title }}

SOURCE:
{{ source_excerpt }}

TRANSLATION:
{{ excerpt }}
"#;

/// SHA-256 of the summarizer templates earlier releases wrote into project
/// snapshots (see [`crate::util::ensure_prompt_file`]).
const SHIPPED_SYSTEM_HASHES: &[&str] =
    &["1a5b98b8f29b0c4aa657993a284bdc4c7b30e4f1a6703c7d13b31f4044b57d23"];
const SHIPPED_USER_HASHES: &[&str] =
    &["35fb639ffde848f131f5f826a8f3065ea0f87a7df63c1460071758a25688c255"];

pub const DEFAULT_SUMMARIZER_SCHEMA: &str = r##"{"$comment":"Rolling-memory schema for prompts/summarizer.md (PLAN.md section 8). The control plane clamps every value again before persisting it: the summary to 200 words, the term list to 8 entries, the style notes to 8 candidates.","type":"object","properties":{"summary":{"type":"string","maxLength":1600},"new_terms":{"type":"array","maxItems":8,"items":{"type":"object","properties":{"source":{"type":"string","maxLength":120},"target":{"type":"string","maxLength":120},"kind":{"enum":["term","proper_noun","do_not_translate"]},"note":{"type":"string","maxLength":200}},"required":["source","target","kind"]}},"style_notes":{"type":"array","maxItems":8,"items":{"type":"string","maxLength":240}}},"required":["summary","new_terms","style_notes"]}"##;

/// Payload of a `summarize` job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummarizePayload {
    pub chapter_id: String,
    /// `true` for the run that closes the chapter.
    #[serde(rename = "final", default)]
    pub final_run: bool,
}

/// Result of one summarization run.
#[derive(Debug, Clone, Serialize)]
pub struct SummarizeOutcome {
    pub chapter_id: String,
    pub final_run: bool,
    pub terms_added: usize,
    /// Total number of stored style-note candidates, not just the new ones.
    pub style_notes: usize,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct RawSummary {
    #[serde(default)]
    summary: String,
    #[serde(default)]
    new_terms: Vec<RawTerm>,
    #[serde(default)]
    style_notes: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct RawTerm {
    #[serde(default)]
    source: String,
    #[serde(default)]
    target: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    note: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CandidateTerm {
    source: String,
    target: String,
    kind: String,
    note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Summary {
    text: String,
    terms: Vec<CandidateTerm>,
    style_notes: Vec<String>,
}

// ---------------------------------------------------------------------------
// Sanitizing
// ---------------------------------------------------------------------------

fn normalize_kind(raw: &str) -> String {
    match raw.trim().to_lowercase().as_str() {
        "proper_noun" => "proper_noun",
        "do_not_translate" => "do_not_translate",
        _ => "term",
    }
    .to_string()
}

fn sanitize(raw: &RawSummary) -> Result<Summary> {
    let text = clamp_words(&raw.summary, MAX_SUMMARY_WORDS);
    if text.is_empty() {
        return Err(AppError::Invalid(
            "the summarizer returned an empty summary".into(),
        ));
    }

    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut terms = Vec::new();
    for term in &raw.new_terms {
        let source = clamp_chars(&term.source, MAX_TERM_CHARS);
        if source.is_empty() || !seen.insert(source.to_lowercase()) {
            continue;
        }
        let kind = normalize_kind(&term.kind);
        let target = clamp_chars(&term.target, MAX_TERM_CHARS);
        let target = if target.is_empty() && kind == "do_not_translate" {
            source.clone()
        } else if target.is_empty() {
            // A candidate without a target cannot enter the prompt.
            continue;
        } else {
            target
        };
        let note = clamp_chars(&term.note, MAX_NOTE_CHARS);
        terms.push(CandidateTerm {
            source,
            target,
            kind,
            note: (!note.is_empty()).then_some(note),
        });
        if terms.len() >= MAX_TERMS {
            break;
        }
    }

    Ok(Summary {
        text,
        terms,
        style_notes: clamp_list(&raw.style_notes, MAX_STYLE_NOTES, MAX_STYLE_NOTE_CHARS),
    })
}

/// Pull the JSON object out of a reply that may carry code fences or prose.
fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| &text[start..=end])
}

fn parse_summary(text: &str) -> Result<Summary> {
    let json = extract_json_object(text)
        .ok_or_else(|| AppError::Invalid("the summarizer answer contains no JSON object".into()))?;
    let raw: RawSummary = serde_json::from_str(json).map_err(|error| {
        AppError::Invalid(format!(
            "the summarizer answer is not a valid summary: {error}"
        ))
    })?;
    sanitize(&raw)
}

// ---------------------------------------------------------------------------
// Excerpt
// ---------------------------------------------------------------------------

/// Keep the head and the tail of a long excerpt: the beginning identifies the
/// chapter, the end is where the translation actually is.
fn clamp_excerpt(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let head_len = max * 2 / 5;
    let tail_len = max - head_len;
    let head: String = text.chars().take(head_len).collect();
    let tail: String = text.chars().skip(count - tail_len).collect();
    format!("{}\n\n[…]\n\n{}", head.trim_end(), tail.trim_start())
}

/// The chunks of a chapter that carry a translation, in order. The summarizer sees
/// their source next to their translation: a glossary pair needs both sides.
fn translated_chunks<'a>(chunks: &'a [Chunk], chapter_id: &str) -> Vec<&'a Chunk> {
    chunks
        .iter()
        .filter(|chunk| chunk.chapter_id.as_deref() == Some(chapter_id))
        .filter(|chunk| crate::pipeline::export::chunk_has_translation(chunk))
        .collect()
}

/// Non-blank texts joined in order.
fn join_texts<'a>(texts: impl IntoIterator<Item = &'a str>) -> String {
    texts
        .into_iter()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Case- and apostrophe-insensitive form used to look a term up in the source.
fn fold(text: &str) -> String {
    text.replace('\u{2019}', "'").to_lowercase()
}

/// Keep only the candidates whose source the chapter actually contains. A source
/// that is not in the source text is the model echoing its own translation, which
/// would ask the translator to keep a word in the wrong language.
fn terms_found_in_source(terms: Vec<CandidateTerm>, source_text: &str) -> Vec<CandidateTerm> {
    let haystack = fold(source_text);
    terms
        .into_iter()
        .filter(|term| haystack.contains(&fold(&term.source)))
        .collect()
}

// ---------------------------------------------------------------------------
// Prompt files
// ---------------------------------------------------------------------------

/// Write the shipped summarizer prompt files into a project snapshot, but never
/// overwrite an edited one.
pub async fn ensure_prompt_files(dir: &Path) -> Result<()> {
    tokio::fs::create_dir_all(dir).await?;
    for (name, content, shipped) in [
        (
            "summarizer.system.md",
            DEFAULT_SUMMARIZER_SYSTEM_TEMPLATE,
            SHIPPED_SYSTEM_HASHES,
        ),
        (
            "summarizer.user.md",
            DEFAULT_SUMMARIZER_USER_TEMPLATE,
            SHIPPED_USER_HASHES,
        ),
        ("summarizer.schema.json", DEFAULT_SUMMARIZER_SCHEMA, &[]),
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

fn load_templates(dir: &Path) -> (String, String) {
    let system = read_first(dir, &["summarizer.system.md"])
        .unwrap_or_else(|| DEFAULT_SUMMARIZER_SYSTEM_TEMPLATE.to_string());
    let user = read_first(dir, &["summarizer.user.md"])
        .unwrap_or_else(|| DEFAULT_SUMMARIZER_USER_TEMPLATE.to_string());
    (system, user)
}

fn load_schema(dir: &Path) -> Value {
    read_first(dir, &["summarizer.schema.json"])
        .and_then(|text| serde_json::from_str(&text).ok())
        // The embedded schema is a compile-time constant: parsing cannot fail.
        .unwrap_or_else(|| {
            serde_json::from_str(DEFAULT_SUMMARIZER_SCHEMA).expect("embedded schema is valid JSON")
        })
}

// ---------------------------------------------------------------------------
// Enqueueing
// ---------------------------------------------------------------------------

/// Enqueue a summary job when the chapter's progress warrants one: every
/// [`ROLLING_EVERY`] completed chunks, and once when no unresolved chunk is
/// left. Idempotent: an equivalent pending job suppresses the enqueue.
///
/// Returns the new job id, or `None` when nothing should run. Without an
/// `orchestrator` binding the whole mechanism is skipped and translation
/// continues unchanged.
pub async fn maybe_enqueue_summaries(
    pool: &SqlitePool,
    project_id: &str,
    chapter_id: &str,
) -> Result<Option<String>> {
    if repo::role_binding_for(pool, ROLE).await?.is_none() {
        return Ok(None);
    }

    let (total, done, unresolved): (i64, i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), \
                COUNT(CASE WHEN status = 'done' THEN 1 END), \
                COUNT(CASE WHEN status NOT IN ('done', 'needs_review') THEN 1 END) \
         FROM chunk WHERE chapter_id = ?1",
    )
    .bind(chapter_id)
    .fetch_one(pool)
    .await?;
    if total == 0 || done == 0 {
        return Ok(None);
    }

    let final_run = unresolved == 0;
    if !final_run && done % ROLLING_EVERY != 0 {
        return Ok(None);
    }

    let payload = serde_json::to_value(SummarizePayload {
        chapter_id: chapter_id.to_string(),
        final_run,
    })?;
    if crate::scheduler::queue::has_pending(pool, project_id, JOB_KIND, &payload).await? {
        return Ok(None);
    }

    let job = crate::scheduler::NewJob::new(project_id, JOB_KIND, payload).with_priority(50);
    let job_id = crate::scheduler::queue::enqueue(pool, &job).await?;
    tracing::debug!(
        project_id,
        chapter_id,
        final_run,
        "enqueued chapter summary"
    );
    Ok(Some(job_id))
}

// ---------------------------------------------------------------------------
// Run
// ---------------------------------------------------------------------------

/// Stored style-note candidates, for the profile snapshot.
pub async fn stored_style_notes(pool: &SqlitePool, project_id: &str) -> Result<Vec<String>> {
    Ok(repo::get_memory(pool, project_id, STYLE_NOTES_KEY)
        .await?
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default())
}

/// Run the summarization and persist its effects.
pub async fn run_summarize(
    deps: &PipelineDeps,
    job_id: Option<&str>,
    project_id: &str,
    payload: &SummarizePayload,
) -> Result<SummarizeOutcome> {
    let pool = &deps.pool;
    let project = repo::get_project(pool, project_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("project {project_id}")))?;
    let chapter = repo::get_chapter(pool, &payload.chapter_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("chapter {}", payload.chapter_id)))?;

    let chunks = repo::list_chunks(pool, &chapter.document_id).await?;
    let translated = translated_chunks(&chunks, &payload.chapter_id);
    if translated.is_empty() {
        return Err(AppError::Invalid(
            "no translated text to summarise yet".into(),
        ));
    }
    let source_text = join_texts(translated.iter().map(|chunk| chunk.source_md.as_str()));
    if source_text.chars().count() < MIN_SOURCE_CHARS {
        tracing::debug!(chapter_id = %payload.chapter_id, "chapter too short to summarise");
        return Ok(SummarizeOutcome {
            chapter_id: payload.chapter_id.clone(),
            final_run: payload.final_run,
            terms_added: 0,
            style_notes: stored_style_notes(pool, project_id).await?.len(),
        });
    }
    let source_excerpt = clamp_excerpt(&source_text, MAX_EXCERPT_CHARS / 2);
    let excerpt = clamp_excerpt(
        &join_texts(
            translated
                .iter()
                .filter_map(|chunk| chunk.target_md.as_deref()),
        ),
        MAX_EXCERPT_CHARS / 2,
    );

    let binding = repo::role_binding_for(pool, ROLE)
        .await?
        .ok_or_else(|| AppError::Invalid("no role_binding configured for 'orchestrator'".into()))?;
    let endpoint = repo::get_endpoint(pool, &binding.endpoint_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("endpoint {}", binding.endpoint_id)))?;

    let prompts_dir = deps.prompts_dir(project_id);
    ensure_prompt_files(&prompts_dir).await?;
    let (system_template, user_template) = load_templates(&prompts_dir);
    let schema = load_schema(&prompts_dir);

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
            chapter_title => &chapter.title,
            source_excerpt => &source_excerpt,
            excerpt => &excerpt,
        },
    )?;
    let prompt_hash = sha256_hex_str(&format!("{system}\n\u{0}\n{user}"));

    let (_, summary) = run_structured_call(
        deps,
        &ChatCall {
            job_id,
            chunk_id: None,
            role: ROLE,
            endpoint_id: &endpoint.id,
            base_url: &endpoint.base_url,
            model: &binding.model,
            params_json: &binding.params_json,
            prompt_hash: &prompt_hash,
            system: &system,
            user: &user,
            response_format: Some(crate::llm::ResponseFormat::json_schema(SCHEMA_NAME, schema)),
            seed: crate::pipeline::translate::derive_seed(
                &format!("{}:{}", payload.chapter_id, payload.final_run),
                ROLE,
            ),
            default_max_tokens: Some(DEFAULT_MAX_TOKENS),
        },
        parse_summary,
    )
    .await?;

    if payload.final_run {
        repo::update_chapter_summary(
            pool,
            &payload.chapter_id,
            &summary.text,
            &binding.model,
            &sha256_hex_str(&excerpt),
        )
        .await?;
        // The chapter summary takes over for the next chapters, so this chapter's rolling
        // summary is cleared. Only its own key is touched: another chapter's summary must
        // survive a concurrent final run.
        let key = rolling_summary_key(Some(&payload.chapter_id));
        repo::set_memory(pool, project_id, &key, "").await?;
    } else {
        let key = rolling_summary_key(Some(&payload.chapter_id));
        repo::set_memory(pool, project_id, &key, &summary.text).await?;
    }

    let proposed = summary.terms.len();
    let terms = terms_found_in_source(summary.terms, &source_text);
    if terms.len() < proposed {
        tracing::info!(
            chapter_id = %payload.chapter_id,
            dropped = proposed - terms.len(),
            "dropped glossary proposals whose source is not in the chapter"
        );
    }
    let terms_added = add_candidates(pool, &project, &terms).await?;
    let style_notes = merge_style_notes(pool, project_id, &summary.style_notes).await?;

    Ok(SummarizeOutcome {
        chapter_id: payload.chapter_id.clone(),
        final_run: payload.final_run,
        terms_added,
        style_notes,
    })
}

/// Insert proposed terms without ever overwriting a different rendering: a
/// conflict marks the existing candidate and records one open finding.
async fn add_candidates(
    pool: &SqlitePool,
    project: &Project,
    terms: &[CandidateTerm],
) -> Result<usize> {
    let mut added = 0;
    let mut conflicts = 0;
    for term in terms {
        let outcome = super::glossary::record_proposal(
            pool,
            project,
            &super::glossary::ProposedTerm {
                source: term.source.clone(),
                target: term.target.clone(),
                kind: term.kind.clone(),
                note: term.note.clone(),
            },
            "proposed",
        )
        .await?;
        match outcome {
            super::glossary::ProposalOutcome::Added => added += 1,
            super::glossary::ProposalOutcome::Conflict => conflicts += 1,
            super::glossary::ProposalOutcome::Unchanged => {}
        }
    }
    if conflicts > 0 {
        tracing::info!(
            project_id = %project.id,
            conflicts,
            "glossary proposals conflicted with existing renderings"
        );
    }
    Ok(added)
}

/// Merge new style notes into the stored candidates, deduped and capped.
/// Returns the total number of stored candidates.
async fn merge_style_notes(pool: &SqlitePool, project_id: &str, notes: &[String]) -> Result<usize> {
    let mut stored = stored_style_notes(pool, project_id).await?;
    for note in notes {
        if !stored
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(note))
        {
            stored.push(note.clone());
        }
    }
    stored.truncate(MAX_STORED_STYLE_NOTES);
    repo::set_memory(
        pool,
        project_id,
        STYLE_NOTES_KEY,
        &serde_json::to_string(&stored)?,
    )
    .await?;
    Ok(stored.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: &str, chapter_id: Option<&str>, order: i64, target: Option<&str>) -> Chunk {
        Chunk {
            id: id.to_string(),
            document_id: "d".to_string(),
            chapter_id: chapter_id.map(str::to_string),
            order_index: order,
            block_ids_json: "[]".to_string(),
            source_md: String::new(),
            token_estimate: 0,
            context_json: "{}".to_string(),
            flags_json: "[]".to_string(),
            status: if target.is_some() { "done" } else { "pending" }.to_string(),
            prompt_hash: None,
            model_id: None,
            params_json: None,
            context_manifest_json: None,
            target_md: target.map(str::to_string),
            error: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn rolling_summary_key_is_scoped_to_the_chapter() {
        assert_ne!(
            rolling_summary_key(Some("ch1")),
            rolling_summary_key(Some("ch2"))
        );
        assert_eq!(rolling_summary_key(Some("ch1")), "rolling_summary:ch1");
        // A chunk outside any chapter still gets a deterministic, distinct key.
        assert_eq!(rolling_summary_key(None), "rolling_summary:__none__");
    }

    #[test]
    fn only_translated_chunks_of_the_chapter_feed_the_excerpts() {
        let chunks = vec![
            chunk("c1", Some("ch"), 1, Some("Primo paragrafo.")),
            chunk("c2", Some("ch"), 2, None),
            chunk("c3", Some("other"), 3, Some("Altro capitolo.")),
            chunk("c4", Some("ch"), 4, Some("Secondo paragrafo.")),
        ];
        let translated = translated_chunks(&chunks, "ch");
        let ids: Vec<&str> = translated.iter().map(|chunk| chunk.id.as_str()).collect();
        assert_eq!(ids, ["c1", "c4"]);
        assert_eq!(
            join_texts(
                translated
                    .iter()
                    .filter_map(|chunk| chunk.target_md.as_deref())
            ),
            "Primo paragrafo.\n\nSecondo paragrafo."
        );
    }

    #[test]
    fn a_proposal_whose_source_is_not_in_the_chapter_is_dropped() {
        let term = |source: &str, target: &str| CandidateTerm {
            source: source.into(),
            target: target.into(),
            kind: "term".into(),
            note: None,
        };
        let source = "Les Figés attendaient. Duom Nil\u{2019} Erg se tut. L'Art du Dessin.";
        let kept = terms_found_in_source(
            vec![
                term("Figés", "the Frozen"),
                // The model echoed its translation as the source.
                term("Art of Drawing", "Art of Drawing"),
                // Case and typographic apostrophes do not matter.
                term("duom nil' erg", "Duom Nil' Erg"),
            ],
            source,
        );
        let sources: Vec<&str> = kept.iter().map(|term| term.source.as_str()).collect();
        assert_eq!(sources, ["Figés", "duom nil' erg"]);
    }

    #[tokio::test]
    async fn a_shipped_template_is_upgraded_and_an_edited_one_is_kept() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A snapshot written by an earlier release (the hash in SHIPPED_SYSTEM_HASHES).
        let old_system = "You maintain the memory of a translation project ({{ source_language }} → {{ target_language }}).
From the chapter excerpt below produce JSON only:
{\"summary\": \"3-5 sentences in {{ target_language }}\",
 \"new_terms\": [{\"source\":\"\",\"target\":\"\",\"kind\":\"term|proper_noun|do_not_translate\",\"note\":\"\"}],
 \"style_notes\": [\"short observations about register, recurring constructions, forms of address\"]}
Output at most 8 new_terms, only terms that recur or matter.";
        std::fs::write(dir.path().join("summarizer.system.md"), old_system).expect("write");
        std::fs::write(dir.path().join("summarizer.user.md"), "my own prompt").expect("write");

        ensure_prompt_files(dir.path()).await.expect("ensure");

        let read = |name: &str| std::fs::read_to_string(dir.path().join(name)).expect("read");
        assert_eq!(
            read("summarizer.system.md"),
            DEFAULT_SUMMARIZER_SYSTEM_TEMPLATE
        );
        assert_eq!(read("summarizer.user.md"), "my own prompt");
    }

    #[test]
    fn clamp_excerpt_keeps_head_and_tail() {
        let text: String = (0..1000)
            .map(|index| char::from(b'a' + (index % 26) as u8))
            .collect();
        let clamped = clamp_excerpt(&text, 100);
        assert!(clamped.chars().count() <= 100 + "\n\n[…]\n\n".chars().count());
        assert!(clamped.starts_with(&text[..10]));
        assert!(clamped.ends_with(&text[text.len() - 10..]));
        assert!(clamped.contains('…'));
        // Short text is returned unchanged.
        assert_eq!(clamp_excerpt("short", 100), "short");
    }

    #[test]
    fn sanitize_clamps_and_rejects_unusable_terms() {
        let raw = RawSummary {
            summary: "word ".repeat(400),
            new_terms: vec![
                RawTerm {
                    source: "keeper".into(),
                    target: "guardiano".into(),
                    kind: "term".into(),
                    note: "n".repeat(400),
                },
                RawTerm {
                    source: "keeper".into(),
                    target: "custode".into(),
                    kind: "term".into(),
                    note: String::new(),
                },
                RawTerm {
                    source: "no target".into(),
                    target: "  ".into(),
                    kind: "term".into(),
                    note: String::new(),
                },
                RawTerm {
                    source: "Harbour Light".into(),
                    target: String::new(),
                    kind: "DO_NOT_TRANSLATE".into(),
                    note: String::new(),
                },
            ],
            style_notes: (0..12).map(|index| format!("note {index}")).collect(),
        };
        let summary = sanitize(&raw).expect("sanitize");

        assert!(summary.text.split_whitespace().count() <= MAX_SUMMARY_WORDS);
        assert_eq!(
            summary.terms.len(),
            2,
            "dedupe by source, skip empty target"
        );
        assert_eq!(summary.terms[0].source, "keeper");
        assert_eq!(summary.terms[0].kind, "term");
        assert!(summary.terms[0]
            .note
            .as_ref()
            .is_some_and(|note| note.chars().count() <= MAX_NOTE_CHARS));
        // A do_not_translate term without a target falls back to its source.
        assert_eq!(summary.terms[1].source, "Harbour Light");
        assert_eq!(summary.terms[1].target, "Harbour Light");
        assert_eq!(summary.terms[1].kind, "do_not_translate");
        assert_eq!(summary.style_notes.len(), MAX_STYLE_NOTES);
    }

    #[test]
    fn sanitize_rejects_an_empty_summary() {
        let raw = RawSummary {
            summary: "   ".into(),
            ..RawSummary::default()
        };
        assert!(matches!(sanitize(&raw), Err(AppError::Invalid(_))));
    }

    #[test]
    fn parse_summary_extracts_json_from_code_fences() {
        let answer =
            "```json\n{\"summary\":\"Un riassunto.\",\"new_terms\":[],\"style_notes\":[]}\n```";
        let summary = parse_summary(answer).expect("parse");
        assert_eq!(summary.text, "Un riassunto.");
        assert!(summary.terms.is_empty());
    }

    #[test]
    fn embedded_prompts_match_the_repository_files() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let text = std::fs::read_to_string(root.join("prompts/summarizer.md"))
            .expect("read prompts/summarizer.md");
        let body = text
            .split_once("-->\n")
            .expect("the file must start with an HTML header")
            .1;
        let (system, user) = body
            .split_once("\n---USER---\n")
            .expect("the file must contain the ---USER--- marker");
        assert_eq!(system, DEFAULT_SUMMARIZER_SYSTEM_TEMPLATE);
        assert_eq!(user, DEFAULT_SUMMARIZER_USER_TEMPLATE);

        let file: Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("prompts/summarizer.schema.json"))
                .expect("read prompts/summarizer.schema.json"),
        )
        .expect("the schema file is valid JSON");
        let embedded: Value =
            serde_json::from_str(DEFAULT_SUMMARIZER_SCHEMA).expect("embedded schema is valid JSON");
        assert_eq!(file, embedded);
    }
}
