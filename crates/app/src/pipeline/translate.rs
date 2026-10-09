//! Translation of a single chunk: context assembly, two-level cache, streaming
//! inference, placeholder validation, persistence and audit.

use std::collections::HashMap;

use serde::Serialize;
use serde_json::Value;

use super::chat_call::{run_chat_call, ChatCall};
use super::PipelineDeps;
use crate::context::budget::CachedCounter;
use crate::context::builder::{ContextBuilder, ContextInputs, GlossaryEntry};
use crate::db::models::{Block, BlockTranslation, QaFinding};
use crate::db::repo::{self, ChunkOutcome};
use crate::db::{new_id, now};
use crate::error::{AppError, Result};
use crate::llm::LlamaClient;
use crate::util::{json_hash, sha256_hex_str};

const ROLE: &str = "translator";

/// Result of translating one chunk.
#[derive(Debug, Clone, Serialize)]
pub struct TranslateOutcome {
    pub chunk_id: String,
    pub status: String,
    pub from_cache: bool,
    pub needs_review_reason: Option<String>,
}

/// Translate a chunk. Idempotent: re-running a completed chunk overwrites the
/// same `block_translation` rows (keyed on `block_id` + `origin`) and the same
/// `chunk` outcome.
///
/// Chunk lifecycle: the chunk is marked `running` as soon as its translate job
/// starts, moves to `done`/`needs_review` at the end (see `finish_chunk`) and to
/// `failed` when the attempt errors out or leaves the chunk with no usable text. A chunk left `running` by a crash is
/// returned to `pending` at boot (`repo::reset_running_chunks`).
pub async fn run_translate_chunk(
    deps: &PipelineDeps,
    job_id: Option<&str>,
    chunk_id: &str,
) -> Result<TranslateOutcome> {
    repo::set_chunk_status(&deps.pool, chunk_id, "running").await?;
    match translate_chunk_inner(deps, job_id, chunk_id).await {
        Ok(outcome) => Ok(outcome),
        Err(error) => {
            // Surface the failed attempt; the job may still be retried, in which
            // case the next run resets the chunk to `running`.
            tracing::warn!(chunk_id, %error, "chunk translation failed");
            if let Err(mark_error) = repo::set_chunk_status(&deps.pool, chunk_id, "failed").await {
                tracing::warn!(chunk_id, %mark_error, "could not mark chunk failed");
            }
            Err(error)
        }
    }
}

async fn translate_chunk_inner(
    deps: &PipelineDeps,
    job_id: Option<&str>,
    chunk_id: &str,
) -> Result<TranslateOutcome> {
    let pool = &deps.pool;
    let chunk = repo::get_chunk(pool, chunk_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("chunk {chunk_id}")))?;
    // The translation already on record, when there is one: a refused attempt must
    // not erase it (see the outcome at the end of this function).
    let previous_target = chunk
        .target_md
        .clone()
        .filter(|text| !text.trim().is_empty());

    let (project_id,): (String,) = sqlx::query_as("SELECT project_id FROM document WHERE id = ?1")
        .bind(&chunk.document_id)
        .fetch_one(pool)
        .await?;
    let project = repo::get_project(pool, &project_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("project {project_id}")))?;

    let binding = repo::role_binding_for(pool, ROLE)
        .await?
        .ok_or_else(|| AppError::Invalid("no role_binding configured for 'translator'".into()))?;
    let endpoint = repo::get_endpoint(pool, &binding.endpoint_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("endpoint {}", binding.endpoint_id)))?;

    let params: Value = serde_json::from_str(&binding.params_json).unwrap_or(Value::Null);
    let params_hash = json_hash(&params);
    let target_lang = project.target_lang.clone();
    let model = binding.model.clone();

    // ---- Blocks ---------------------------------------------------------
    let all_blocks = repo::list_blocks(pool, &chunk.document_id).await?;
    let by_id: HashMap<&str, &Block> = all_blocks.iter().map(|b| (b.id.as_str(), b)).collect();
    let block_ids: Vec<String> = serde_json::from_str(&chunk.block_ids_json).unwrap_or_default();
    let chunk_blocks: Vec<&Block> = block_ids
        .iter()
        .filter_map(|id| by_id.get(id.as_str()).copied())
        .collect();
    // ---- Effective glossary: project terms override the series canon -----
    // Resolved before the memory check because the effective glossary is part of the
    // memory identity: a canon change must not keep serving old renderings.
    let glossary_terms = crate::pipeline::glossary::effective_terms(pool, &project_id).await?;
    let glossary_hash = crate::pipeline::glossary::effective_glossary_hash(&glossary_terms);

    // ---- Two-level cache: block-level memory first ----------------------
    if let Some(reused) = try_memory_reuse(
        deps,
        &chunk.id,
        &chunk_blocks,
        &model,
        &target_lang,
        &glossary_hash,
    )
    .await?
    {
        // Compose the chunk's markdown from the rows just written: each
        // translatable block takes its reused translation and every other block
        // its own source. Persisting the chunk source as a whole (the old
        // behaviour) marked a chunk `done` with an untranslated body.
        let translations = translations_from_map(&chunk_blocks, &reused);
        let target_md = compose_target_md(&chunk_blocks, &translations);
        let outcome = ChunkOutcome {
            chunk_id: chunk.id.clone(),
            status: "done".into(),
            prompt_hash: None,
            model_id: Some(model),
            params_json: Some(binding.params_json.clone()),
            context_manifest_json: None,
            target_md: Some(target_md),
            error: None,
        };
        repo::finish_chunk(pool, &outcome).await?;
        tracing::info!(chunk_id = %chunk.id, status = "done", from_cache = true, "chunk settled");
        return Ok(TranslateOutcome {
            chunk_id: chunk.id.clone(),
            status: "done".into(),
            from_cache: true,
            needs_review_reason: None,
        });
    }

    // ---- Context --------------------------------------------------------
    let prompts_dir = deps.prompts_dir(&project_id);
    let builder = ContextBuilder::load(&prompts_dir);
    let glossary: Vec<GlossaryEntry> = glossary_terms
        .into_iter()
        .map(|term| GlossaryEntry {
            source: term.source,
            target: term.target,
            kind: term.kind,
            aliases: term.aliases,
        })
        .collect();
    let dialogue_quotes = crate::pipeline::recon::wants_dialogue_quotes(
        &crate::pipeline::glossary::memory_with_series(
            pool,
            &project,
            crate::pipeline::recon::DIALOGUE_STYLE_KEY,
        )
        .await?,
    );
    let inputs = ContextInputs {
        source_language: project.source_lang.clone().unwrap_or_default(),
        target_language: target_lang.clone(),
        style_guide: crate::pipeline::glossary::memory_with_series(pool, &project, "style_guide")
            .await?,
        synopsis: crate::pipeline::glossary::memory_with_series(pool, &project, "synopsis").await?,
        book_title: project.doc_title.clone().unwrap_or_default(),
        book_author: project.doc_author.clone().unwrap_or_default(),
        glossary,
        previous_chapter_summaries: previous_chapter_summaries(
            pool,
            &chunk.document_id,
            chunk.chapter_id.as_deref(),
        )
        .await?,
        rolling_summary: repo::get_memory(
            pool,
            &project_id,
            crate::pipeline::summarize::ROLLING_SUMMARY_KEY,
        )
        .await?
        .unwrap_or_default(),
        previous_tail: previous_tail(pool, &chunk.document_id, chunk.order_index).await?,
        chapter_title: chapter_title(pool, chunk.chapter_id.as_deref()).await?,
        heading_chain: heading_chain(&chunk.context_json),
        chunk_flags: describe_chunk_flags(&chunk.flags_json),
        chunk_text: chunk.source_md.clone(),
        budget_tokens: crate::pipeline::resolve_endpoint_budget(&endpoint).await,
        dialogue_quotes,
    };

    // Placeholder preparation happens on the sidecar (pure function).
    let prepared = deps
        .sidecar
        .prepare_text(Some(&block_ids), &chunk.source_md)
        .await?;
    let mut inputs = inputs;
    inputs.chunk_text = prepared.llm_text.clone();

    let (system, pieces) = builder.pieces(&inputs)?;
    // Exact counts from `/tokenize` when the server exposes it; the counter
    // falls back to the heuristic for anything the server did not answer for.
    let texts: Vec<&str> = pieces
        .iter()
        .map(|piece| piece.text.as_str())
        .filter(|text| !text.is_empty())
        .collect();
    let token_counts = match LlamaClient::new(&endpoint.base_url) {
        Ok(client) => client.token_counts(&texts).await,
        Err(_) => std::collections::HashMap::new(),
    };
    let counter = CachedCounter::new(token_counts);
    let built = builder.build_from_pieces(&inputs, system, pieces, &counter)?;
    let prompt_hash = sha256_hex_str(&built.full_text());
    let manifest_json = serde_json::to_string(&built.manifest)?;

    // ---- Exact cache ----------------------------------------------------
    let mut from_cache = false;
    let mut response_text = if let Some(cached) =
        repo::cache_get(pool, &prompt_hash, &model, &params_hash, &target_lang).await?
    {
        from_cache = true;
        cached
    } else {
        String::new()
    };

    let mut needs_review_reason: Option<String> = None;

    if !from_cache {
        response_text = run_chat_call(
            deps,
            &ChatCall {
                job_id,
                chunk_id: Some(&chunk.id),
                role: ROLE,
                endpoint_id: &endpoint.id,
                base_url: &endpoint.base_url,
                model: &model,
                params_json: &binding.params_json,
                prompt_hash: &prompt_hash,
                system: &built.system,
                user: &built.user,
                response_format: None,
                seed: derive_seed(&chunk.id, ROLE),
                default_max_tokens: None,
            },
        )
        .await?;
    }

    // ---- Reinject + placeholder validation ------------------------------
    // With quotation marks for dialogue the model is told to drop the dash token that
    // opens a spoken line, so those tokens alone may go missing.
    let optional_tokens = if dialogue_quotes {
        dialogue_dash_tokens(&prepared.placeholders)
    } else {
        Vec::new()
    };
    let mut reinject = deps
        .sidecar
        .reinject(&response_text, &prepared.placeholders, block_ids.len())
        .await?;
    let mut placeholders_ok = placeholders_acceptable(&reinject, &optional_tokens);

    if !placeholders_ok {
        // One targeted retry naming the tokens that came back wrong (missing, duplicated
        // or invented by the model).
        let missing: Vec<u32> = reinject
            .missing
            .iter()
            .copied()
            .filter(|index| !optional_tokens.contains(index))
            .collect();
        let broken = describe_placeholders(&missing, &reinject.duplicated, &reinject.unknown);
        let retry_user = format!(
            "{}\n\nIMPORTANT: your previous answer was rejected because these placeholder tokens \
             were missing, duplicated or unknown: {broken}. Re-output the passage, including \
             every one of them exactly once and inventing no new ones.",
            built.user
        );
        let retry_hash = sha256_hex_str(&format!("{}\n\u{0}\n{}", built.system, retry_user));
        response_text = run_chat_call(
            deps,
            &ChatCall {
                job_id,
                chunk_id: Some(&chunk.id),
                role: ROLE,
                endpoint_id: &endpoint.id,
                base_url: &endpoint.base_url,
                model: &model,
                params_json: &binding.params_json,
                prompt_hash: &retry_hash,
                system: &built.system,
                user: &retry_user,
                response_format: None,
                seed: derive_seed(&chunk.id, ROLE),
                default_max_tokens: None,
            },
        )
        .await?;
        reinject = deps
            .sidecar
            .reinject(&response_text, &prepared.placeholders, block_ids.len())
            .await?;
        placeholders_ok = placeholders_acceptable(&reinject, &optional_tokens);
        if !placeholders_ok {
            needs_review_reason = Some(format!(
                "placeholder validation failed after retry: missing {:?}, duplicated {:?}, unknown {:?}",
                reinject.missing, reinject.duplicated, reinject.unknown
            ));
            write_qa_finding(
                pool,
                &project_id,
                &chunk.id,
                "placeholder_broken",
                serde_json::json!({
                    "missing": reinject.missing,
                    "duplicated": reinject.duplicated,
                    "unknown": reinject.unknown,
                }),
            )
            .await?;
        }
    }

    // ---- Block alignment ------------------------------------------------
    let block_count_ok = reinject.block_count_ok && reinject.blocks_md.len() == block_ids.len();
    if !block_count_ok && needs_review_reason.is_none() {
        // Never align by force: mark for review instead.
        needs_review_reason = Some(format!(
            "block count mismatch: got {} blocks for {} expected",
            reinject.blocks_md.len(),
            block_ids.len()
        ));
        write_qa_finding(
            pool,
            &project_id,
            &chunk.id,
            "markdown_malformed",
            serde_json::json!({
                "expected_blocks": block_ids.len(),
                "got_blocks": reinject.blocks_md.len(),
            }),
        )
        .await?;
    }

    // Persist block translations only when alignment is trustworthy.
    if block_count_ok {
        for (index, block) in chunk_blocks.iter().enumerate() {
            if !block.translatable {
                continue;
            }
            let text_md = reinject.blocks_md.get(index).cloned().unwrap_or_default();
            repo::upsert_block_translation(
                pool,
                &BlockTranslation {
                    block_id: block.id.clone(),
                    chunk_id: chunk.id.clone(),
                    text_md,
                    placeholders_ok,
                    origin: ROLE.to_string(),
                    edited_by_user: false,
                    updated_at: now(),
                },
            )
            .await?;
        }
        // Refresh the translation memory for future identical blocks.
        if placeholders_ok {
            for (index, block) in chunk_blocks.iter().enumerate() {
                if !block.translatable {
                    continue;
                }
                if let Some(text_md) = reinject.blocks_md.get(index) {
                    repo::memory_put(
                        pool,
                        &block.content_hash,
                        &model,
                        &target_lang,
                        &glossary_hash,
                        text_md,
                    )
                    .await?;
                }
            }
            repo::cache_put(
                pool,
                &prompt_hash,
                &model,
                &params_hash,
                &target_lang,
                &response_text,
            )
            .await?;
        }
    }

    // Persist the validated, placeholder-free markdown, never the raw model
    // response: the raw text (still holding ⟦n⟧ tokens) is preserved in
    // `llm_call.response_text` for audit. When the block alignment is not
    // trustworthy the new answer is dropped, so the chunk is `needs_review` and
    // export falls back to its source instead of emitting unvalidated text —
    // a LaTeX build must never see a placeholder token.
    let translations: Vec<Option<&str>> = chunk_blocks
        .iter()
        .enumerate()
        .map(|(index, block)| {
            if block.translatable {
                reinject.blocks_md.get(index).map(String::as_str)
            } else {
                None
            }
        })
        .collect();
    let attempted = aligned_target_md(block_count_ok, &chunk_blocks, &translations);
    // A refused attempt never erases a translation that was already validated: the
    // chunk is flagged for review with the reason, but the text the user had stays,
    // so a re-run (a leftover duplicate, a retry) cannot take a good chunk back to
    // "untranslated". The rejected answer is still in `llm_call.response_text`.
    let kept_previous = attempted.is_none() && previous_target.is_some();
    let target_md = attempted.or_else(|| previous_target.clone());

    // Quality scan (M4): advisory findings on the validated markdown, so a
    // re-scan and the inline run agree. A QA failure must never fail a chunk.
    if let Some(target) = target_md.as_deref() {
        if let Err(error) =
            crate::pipeline::qa::scan_translated_chunk(deps, &project_id, &chunk, target).await
        {
            tracing::warn!(chunk_id = %chunk.id, %error, "qa scan failed");
        }
    }

    // `needs_review` always means "translated, check it". An attempt that leaves the
    // chunk with no usable text is `failed`: a resume retries it, and no view or export
    // mistakes it for translated.
    let status = match (&target_md, needs_review_reason.is_some()) {
        (None, _) => "failed",
        (Some(_), true) => "needs_review",
        (Some(_), false) => "done",
    };
    let error = match (needs_review_reason.as_deref(), kept_previous) {
        (Some(reason), true) => Some(format!("{reason}; the previous translation was kept")),
        (reason, _) => reason.map(str::to_string),
    };
    repo::finish_chunk(
        pool,
        &ChunkOutcome {
            chunk_id: chunk.id.clone(),
            status: status.to_string(),
            prompt_hash: Some(prompt_hash),
            model_id: Some(model),
            params_json: Some(binding.params_json.clone()),
            context_manifest_json: Some(manifest_json),
            target_md,
            error,
        },
    )
    .await?;
    tracing::info!(
        chunk_id = %chunk.id,
        status,
        from_cache,
        kept_previous,
        needs_review = needs_review_reason.as_deref().unwrap_or(""),
        "chunk settled"
    );

    // Rolling memory: enqueue a chapter summary once the chapter's progress
    // warrants one (PLAN.md section 8). Without an orchestrator binding this is
    // a no-op and translation continues exactly as before; a failure here must
    // never fail the chunk itself.
    if let Some(chapter_id) = chunk.chapter_id.as_deref() {
        if let Err(error) =
            crate::pipeline::summarize::maybe_enqueue_summaries(pool, &project_id, chapter_id).await
        {
            tracing::warn!(chapter_id, %error, "could not enqueue a chapter summary");
        }
    }

    Ok(TranslateOutcome {
        chunk_id: chunk.id,
        status: status.to_string(),
        from_cache,
        needs_review_reason,
    })
}

/// Try to satisfy a chunk from the block-level translation memory.
///
/// On a full hit it writes one `block_translation` row per translatable block and
/// returns the `block_id -> text_md` map it just persisted, so the caller can
/// compose the chunk's `target_md` from the reused text. On the first miss it
/// returns `None` and the chunk falls through to real inference; the rows already
/// written for earlier blocks are left in place, and the inference path overwrites
/// them with the fresh translation (the upsert is keyed on `block_id` + `origin`).
async fn try_memory_reuse(
    deps: &PipelineDeps,
    chunk_id: &str,
    chunk_blocks: &[&Block],
    model: &str,
    target_lang: &str,
    glossary_hash: &str,
) -> Result<Option<HashMap<String, String>>> {
    let translatable: Vec<&&Block> = chunk_blocks.iter().filter(|b| b.translatable).collect();
    if translatable.is_empty() {
        return Ok(None);
    }
    let mut reused: HashMap<String, String> = HashMap::new();
    for block in translatable {
        match repo::memory_get(
            &deps.pool,
            &block.content_hash,
            model,
            target_lang,
            glossary_hash,
        )
        .await?
        {
            Some(text_md) => {
                repo::upsert_block_translation(
                    &deps.pool,
                    &BlockTranslation {
                        block_id: block.id.clone(),
                        chunk_id: chunk_id.to_string(),
                        text_md: text_md.clone(),
                        placeholders_ok: true,
                        origin: ROLE.to_string(),
                        edited_by_user: false,
                        updated_at: now(),
                    },
                )
                .await?;
                reused.insert(block.id.clone(), text_md);
            }
            None => return Ok(None),
        }
    }
    Ok(Some(reused))
}

/// Compose a chunk's validated, placeholder-free markdown from its blocks, the
/// same way the sidecar's `render()` builds its output: a block contributes its
/// translation when it has one and its own source otherwise, and the blocks are
/// joined with a blank line (how `Chunk.source_md` joins them).
///
/// `translations` is positional: entry `i` corresponds to `chunk_blocks[i]`, and
/// `None` means "no translation for this block".
///
/// Non-translatable blocks (code fences, raw HTML, figures, footnotes, ...) ALWAYS
/// come from `source_md`, never from the model. The model is not asked to translate
/// them, so any text attributed to them would be unvalidated — and a stray rewrite
/// of a code fence or an `<img>` would silently corrupt the output.
fn compose_target_md(chunk_blocks: &[&Block], translations: &[Option<&str>]) -> String {
    let parts: Vec<&str> = chunk_blocks
        .iter()
        .enumerate()
        .map(|(index, block)| {
            if block.translatable {
                translations
                    .get(index)
                    .copied()
                    .flatten()
                    .unwrap_or(block.source_md.as_str())
            } else {
                block.source_md.as_str()
            }
        })
        .collect();
    parts.join("\n\n")
}

/// Positional translations for [`compose_target_md`], derived from a
/// `block_id -> text_md` map (the memory-reuse path, where the reused rows were
/// just written).
fn translations_from_map<'a>(
    chunk_blocks: &[&'a Block],
    reuse: &'a HashMap<String, String>,
) -> Vec<Option<&'a str>> {
    chunk_blocks
        .iter()
        .map(|block| reuse.get(&block.id).map(String::as_str))
        .collect()
}

/// The `target_md` to persist for a chunk: the composed translation when the block
/// alignment is trustworthy (`block_count_ok`), or `None` otherwise. `None` means
/// the chunk is `needs_review`, the raw reply is preserved in
/// `llm_call.response_text`, and export falls back to the chunk's source.
fn aligned_target_md(
    block_count_ok: bool,
    chunk_blocks: &[&Block],
    translations: &[Option<&str>],
) -> Option<String> {
    block_count_ok.then(|| compose_target_md(chunk_blocks, translations))
}

/// Deterministic seed from `hash(chunk_id, role)` (PLAN.md section 10).
pub fn derive_seed(chunk_id: &str, role: &str) -> i64 {
    let hash = sha256_hex_str(&format!("{chunk_id}:{role}"));
    let bytes = hex::decode(&hash[..16]).unwrap_or_default();
    let mut buf = [0u8; 8];
    for (i, b) in bytes.iter().take(8).enumerate() {
        buf[i] = *b;
    }
    i64::from_be_bytes(buf).abs()
}

fn format_placeholders(tokens: &[u32]) -> String {
    if tokens.is_empty() {
        return "(none reported)".to_string();
    }
    tokens
        .iter()
        .map(|t| format!("⟦{t}⟧"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Placeholder indices that stand for the dash opening a dialogue line (`\-`).
fn dialogue_dash_tokens(placeholders: &[(u32, String)]) -> Vec<u32> {
    placeholders
        .iter()
        .filter(|(_, literal)| literal == "\\-")
        .map(|(index, _)| *index)
        .collect()
}

/// Whether a reinjected answer kept its placeholders, allowing `optional` tokens to be
/// missing (and only missing: a duplicated or invented token is still an error).
fn placeholders_acceptable(reinject: &crate::sidecar::ReinjectResult, optional: &[u32]) -> bool {
    reinject.placeholders_ok
        || (reinject.duplicated.is_empty()
            && reinject.unknown.is_empty()
            && reinject
                .missing
                .iter()
                .all(|index| optional.contains(index)))
}

/// One human-readable summary of every placeholder defect a reinject pass reported,
/// for the targeted retry message. Categories with no tokens are omitted.
fn describe_placeholders(missing: &[u32], duplicated: &[u32], unknown: &[u32]) -> String {
    let mut parts = Vec::new();
    if !missing.is_empty() {
        parts.push(format!("missing {}", format_placeholders(missing)));
    }
    if !duplicated.is_empty() {
        parts.push(format!("duplicated {}", format_placeholders(duplicated)));
    }
    if !unknown.is_empty() {
        parts.push(format!("unknown {}", format_placeholders(unknown)));
    }
    if parts.is_empty() {
        "(none reported)".to_string()
    } else {
        parts.join("; ")
    }
}

/// The human-readable title of the chunk's chapter, or an empty string when the
/// chunk belongs to the preamble (no chapter) or the row is missing.
async fn chapter_title(pool: &sqlx::SqlitePool, chapter_id: Option<&str>) -> Result<String> {
    let Some(chapter_id) = chapter_id else {
        return Ok(String::new());
    };
    let title: Option<String> = sqlx::query_scalar("SELECT title FROM chapter WHERE id = ?1")
        .bind(chapter_id)
        .fetch_optional(pool)
        .await?;
    Ok(title.unwrap_or_default())
}

/// The heading chain the chunk sits in, rendered `A › B › C`.
///
/// The chain is what the chunker stored in `context_carrier` (PLAN.md §9.1):
/// `{"headings": [{"level": 2, "text": "Chapter One"}, ...]}`. Malformed JSON is
/// treated as "no headings" rather than failing the translation.
fn heading_chain(context_json: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(context_json) else {
        return String::new();
    };
    let Some(headings) = value.get("headings").and_then(Value::as_array) else {
        return String::new();
    };
    headings
        .iter()
        .filter_map(|heading| heading.get("text").and_then(Value::as_str))
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" › ")
}

/// Turn the chunker's raw flags into the instruction the user message states.
/// Unknown flags are passed through verbatim instead of being dropped, so a new
/// flag from a future chunker still reaches the model.
fn describe_chunk_flags(flags_json: &str) -> String {
    let flags: Vec<String> = serde_json::from_str(flags_json).unwrap_or_default();
    flags
        .iter()
        .map(|flag| match flag.as_str() {
            "table" => "this passage contains a table: keep the pipe structure and the header separator".to_string(),
            "continues" => "this passage continues the previous one (it was split at a sentence boundary): translate it as a continuation".to_string(),
            "oversized" => "this passage is a single block larger than the context budget: translate it completely, without shortening it".to_string(),
            other if other.starts_with("table_part:") => format!(
                "this passage is {other_part} of a table split across chunks: repeat the header row and keep the pipe structure",
                other_part = other.trim_start_matches("table_part:").trim()),
            other => format!("chunk flag: {other}"),
        })
        .collect::<Vec<_>>()
        .join("; ")
}

async fn previous_chapter_summaries(
    pool: &sqlx::SqlitePool,
    document_id: &str,
    chapter_id: Option<&str>,
) -> Result<Vec<String>> {
    let Some(chapter_id) = chapter_id else {
        return Ok(Vec::new());
    };
    let current_order: Option<i64> =
        sqlx::query_scalar("SELECT order_index FROM chapter WHERE id = ?1")
            .bind(chapter_id)
            .fetch_optional(pool)
            .await?;
    let Some(order) = current_order else {
        return Ok(Vec::new());
    };
    let rows: Vec<(Option<String>,)> = sqlx::query_as(
        "SELECT summary FROM chapter WHERE document_id = ?1 AND order_index < ?2 \
         ORDER BY order_index DESC LIMIT 3",
    )
    .bind(document_id)
    .bind(order)
    .fetch_all(pool)
    .await?;
    let mut summaries: Vec<String> = rows.into_iter().filter_map(|r| r.0).collect();
    summaries.reverse();
    Ok(summaries)
}

async fn previous_tail(pool: &sqlx::SqlitePool, document_id: &str, order: i64) -> Result<String> {
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT target_md FROM chunk WHERE document_id = ?1 AND order_index < ?2 \
         AND target_md IS NOT NULL ORDER BY order_index DESC LIMIT 1",
    )
    .bind(document_id)
    .bind(order)
    .fetch_optional(pool)
    .await?;
    Ok(row
        .and_then(|r| r.0)
        .map(|text| tail_chars(&text, 800))
        .unwrap_or_default())
}

fn tail_chars(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.to_string();
    }
    chars[chars.len() - max..].iter().collect()
}

async fn write_qa_finding(
    pool: &sqlx::SqlitePool,
    project_id: &str,
    chunk_id: &str,
    kind: &str,
    details: Value,
) -> Result<()> {
    repo::insert_qa_finding(
        pool,
        &QaFinding {
            id: new_id(),
            project_id: project_id.to_string(),
            chunk_id: Some(chunk_id.to_string()),
            block_id: None,
            kind: kind.to_string(),
            severity: "major".to_string(),
            details_json: serde_json::to_string(&details)?,
            status: "open".to_string(),
            created_at: now(),
        },
    )
    .await
}

/// Exposed for tests: the manifest type is persisted as JSON on the chunk.
#[cfg(test)]
fn manifest_to_json(manifest: &crate::context::budget::BudgetedContext) -> Result<String> {
    Ok(serde_json::to_string(manifest)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_dialogue_dash_tokens_may_go_missing() {
        let placeholders = vec![
            (1, "\\-".to_string()),
            (2, "**".to_string()),
            (3, "\\-".to_string()),
        ];
        let optional = dialogue_dash_tokens(&placeholders);
        assert_eq!(optional, [1, 3]);

        let result = |missing: Vec<u32>, duplicated: Vec<u32>| crate::sidecar::ReinjectResult {
            placeholders_ok: missing.is_empty() && duplicated.is_empty(),
            missing,
            duplicated,
            ..Default::default()
        };
        assert!(placeholders_acceptable(&result(vec![], vec![]), &optional));
        // The dashes became quotation marks: fine.
        assert!(placeholders_acceptable(
            &result(vec![1, 3], vec![]),
            &optional
        ));
        // Losing real markup is still an error, so is a duplicated dash.
        assert!(!placeholders_acceptable(
            &result(vec![2], vec![]),
            &optional
        ));
        assert!(!placeholders_acceptable(
            &result(vec![], vec![1]),
            &optional
        ));
        // Without the quotes convention nothing is optional.
        assert!(!placeholders_acceptable(&result(vec![1], vec![]), &[]));
    }
    use crate::context::budget::{BudgetedContext, PieceKind, PieceReport};
    use crate::db::models::Block;

    /// A minimal `Block` for the composition tests.
    fn test_block(id: &str, kind: &str, translatable: bool, source_md: &str) -> Block {
        Block {
            id: id.to_string(),
            document_id: "d".into(),
            chapter_id: None,
            order_index: 0,
            kind: kind.into(),
            level: 0,
            source_md: source_md.into(),
            source_text: source_md.into(),
            translatable,
            attrs_json: "{}".into(),
            content_hash: format!("h-{id}"),
        }
    }

    #[test]
    fn compose_target_md_uses_translation_and_keeps_non_translatable_from_source() {
        let para = test_block("b1", "para", true, "The old harbour");
        let code = test_block("b2", "code", false, "```sql\nSELECT 1;\n```");
        let blocks = [&para, &code];
        // The model also returned the code block "translated"; it is ignored
        // because a non-translatable block always comes from its source.
        let translations = [Some("Il vecchio porto"), Some("```sql\nSELECT 2;\n```")];
        let composed = compose_target_md(&blocks, &translations);
        assert_eq!(composed, "Il vecchio porto\n\n```sql\nSELECT 1;\n```");
        assert!(!composed.contains("SELECT 2;"));
        assert!(!composed.contains('\u{27e6}'));
    }

    #[test]
    fn compose_target_md_falls_back_to_source_when_a_translation_is_missing() {
        let first = test_block("b1", "para", true, "Hello");
        let second = test_block("b2", "para", true, "World");
        let blocks = [&first, &second];
        let translations = [Some("Ciao"), None];
        assert_eq!(compose_target_md(&blocks, &translations), "Ciao\n\nWorld");
    }

    #[test]
    fn untrustworthy_alignment_leaves_target_md_null() {
        let para = test_block("b1", "para", true, "Hello");
        let blocks = [&para];
        let translations = [Some("Ciao")];
        // A block-count mismatch means `needs_review` and no target, so export
        // falls back to the source instead of emitting unvalidated text.
        assert_eq!(aligned_target_md(false, &blocks, &translations), None);
        assert_eq!(
            aligned_target_md(true, &blocks, &translations).as_deref(),
            Some("Ciao")
        );
    }

    #[test]
    fn reuse_path_composes_the_reused_text_and_never_the_chunk_source() {
        let para = test_block("b1", "para", true, "The harbour was quiet.");
        let figure = test_block("b2", "figure", false, "![harbour](harbour.png)");
        let blocks = [&para, &figure];
        let mut reuse: HashMap<String, String> = HashMap::new();
        reuse.insert("b1".to_string(), "Il porto era tranquillo.".to_string());

        let translations = translations_from_map(&blocks, &reuse);
        // The figure is not translatable, so the memory never holds a row for it.
        assert_eq!(translations[1], None);
        let composed = compose_target_md(&blocks, &translations);
        assert_eq!(
            composed,
            "Il porto era tranquillo.\n\n![harbour](harbour.png)"
        );
        // The old bug persisted the whole chunk source as the translation.
        let chunk_source = format!("{}\n\n{}", para.source_md, figure.source_md);
        assert_ne!(composed, chunk_source);
    }

    #[test]
    fn describe_placeholders_names_every_defect_category() {
        assert_eq!(describe_placeholders(&[], &[], &[]), "(none reported)");
        assert_eq!(describe_placeholders(&[2], &[], &[]), "missing ⟦2⟧");
        assert_eq!(
            describe_placeholders(&[2], &[3], &[99]),
            "missing ⟦2⟧; duplicated ⟦3⟧; unknown ⟦99⟧"
        );
    }

    #[test]
    fn heading_chain_renders_the_stored_carrier() {
        let carrier = r#"{"headings":[{"level":1,"text":"The Book"},{"level":2,"text":"Chapter One"},{"level":3,"text":"  The Harbour  "}]}"#;
        assert_eq!(
            heading_chain(carrier),
            "The Book › Chapter One › The Harbour"
        );
        // Degenerate inputs never fail a translation.
        assert_eq!(heading_chain("{}"), "");
        assert_eq!(heading_chain("not json"), "");
        assert_eq!(heading_chain(r#"{"headings":[]}"#), "");
    }

    #[test]
    fn chunk_flags_become_instructions_and_unknown_ones_survive() {
        let rendered = describe_chunk_flags(r#"["table","table_part:2/3","continues"]"#);
        assert!(rendered.contains("contains a table"));
        assert!(rendered.contains("2/3 of a table split"));
        assert!(rendered.contains("continues the previous one"));
        assert_eq!(rendered.matches(';').count(), 2);

        // A flag the map does not know is still stated to the model, not dropped.
        assert_eq!(
            describe_chunk_flags(r#"["future_flag"]"#),
            "chunk flag: future_flag"
        );
        assert_eq!(describe_chunk_flags("not json"), "");
        assert_eq!(describe_chunk_flags("[]"), "");
    }

    #[test]
    fn seed_is_deterministic_and_role_specific() {
        assert_eq!(
            derive_seed("c000001", "translator"),
            derive_seed("c000001", "translator")
        );
        assert_ne!(
            derive_seed("c000001", "translator"),
            derive_seed("c000001", "editor")
        );
        assert_ne!(
            derive_seed("c000001", "translator"),
            derive_seed("c000002", "translator")
        );
        assert!(derive_seed("c000001", "translator") >= 0);
    }

    #[test]
    fn manifest_serialises_without_text() {
        let manifest = BudgetedContext {
            pieces: vec![PieceReport {
                name: PieceKind::Text,
                priority: 0,
                tokens: 3,
                included: true,
                truncated: false,
                hash: "abc".into(),
                text: "secret".into(),
            }],
            total_tokens: 3,
            budget_tokens: 100,
        };
        let json = manifest_to_json(&manifest).expect("json");
        assert!(json.contains("abc"));
        assert!(!json.contains("secret"));
    }
}
