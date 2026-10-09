//! Series reconnaissance (PLAN.md §9.5, S5): a candidate series profile.
//!
//! The evidence is local: the confirmed synopsis and style guide of the member books plus
//! the canon glossary. The answer is a **candidate** stored under
//! `series_memory['series_profile']`: it never reaches a translation prompt on its own, the
//! user reviews it and copies what they want into the series style guide or synopsis.

use std::collections::BTreeSet;
use std::path::Path;

use minijinja::{context, Environment};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::chat_call::{run_structured_call, ChatCall};
use super::PipelineDeps;
use crate::db::models::Project;
use crate::db::repo;
use crate::db::{new_id, now};
use crate::error::{AppError, Result};
use crate::util::{clamp_chars, clamp_list, clamp_words, sha256_hex_str};

/// Role binding used for the series synthesis.
pub const ROLE: &str = "orchestrator";
/// Job kind that runs the series reconnaissance.
pub const JOB_KIND: &str = "series_recon";
/// Memory key holding the candidate profile.
pub const MEMORY_KEY: &str = "series_profile";
const SCHEMA_NAME: &str = "series_profile";

const MAX_BOOKS: usize = 12;
const MAX_SYNOPSIS_WORDS: usize = 150;
const MAX_STYLE_NOTES: usize = 8;
const MAX_STYLE_NOTE_CHARS: usize = 240;
const MAX_CHARACTERS: usize = 24;
const MAX_SHORT_CHARS: usize = 120;
const MAX_NOTE_CHARS: usize = 200;
const MAX_EVIDENCE_CHARS: usize = 12_000;
const MAX_GLOSSARY_TERMS: usize = 200;
const DEFAULT_MAX_TOKENS: u32 = 1200;

/// Fallback templates, byte-identical to `prompts/series_recon.md` and
/// `prompts/series_recon.schema.json` (a unit test asserts that).
pub const DEFAULT_SERIES_RECON_SYSTEM_TEMPLATE: &str = r#"You are the canon editor of a translated book series ({{ source_language }} → {{ target_language }}).
You receive the confirmed profiles of the books already translated and the series glossary.
Produce a CANDIDATE series profile: what a translator of the next book must know to stay
consistent with the saga — recurring characters, invented terms, register, running themes.
Reply with JSON only, no commentary, no code fences. Do not invent facts the evidence cannot
support; an empty list is a valid answer."#;

pub const DEFAULT_SERIES_RECON_USER_TEMPLATE: &str = r#"SERIES: {{ series_name }}

{% if previous_profile %}PREVIOUS CANDIDATE PROFILE (update it; keep what is still valid):
{{ previous_profile }}
{% endif %}
BOOKS (confirmed profiles):
{{ books }}

SERIES GLOSSARY (source => target):
{{ glossary }}

Reply with a single JSON object that validates against this schema:
{{ response_schema }}
"#;

pub const DEFAULT_SERIES_RECON_SCHEMA: &str = r##"{"$comment":"Series reconnaissance schema (PLAN.md §9.5, S5). Everything here is a candidate: nothing reaches a translation prompt until the user copies it into the series memory.","type":"object","properties":{"synopsis":{"type":"string","maxLength":1200},"style_notes":{"type":"array","maxItems":8,"items":{"type":"string","maxLength":240}},"characters":{"type":"array","maxItems":24,"items":{"type":"object","properties":{"source":{"type":"string","maxLength":120},"target":{"type":"string","maxLength":120},"note":{"type":"string","maxLength":200}},"required":["source","target"]}}},"required":["synopsis","style_notes","characters"]}"##;

/// One recurring character or term of the saga.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, ts_rs::TS)]
#[ts(export)]
pub struct SeriesCharacter {
    pub source: String,
    pub target: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, ts_rs::TS)]
#[ts(export)]
pub struct SeriesProfileProvenance {
    #[serde(default)]
    pub generated_at: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub prompt_hash: String,
    /// Names of the books the profile was derived from.
    #[serde(default)]
    pub books: Vec<String>,
    /// Identity of every book's evidence, so an unchanged book is not re-synthesized.
    #[serde(default)]
    pub sources: Vec<BookSource>,
    /// Hash of the canon the profile was built with: a glossary change invalidates it.
    #[serde(default)]
    pub glossary_hash: String,
}

/// One book's evidence identity (`project_id` + hash of its confirmed profile).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, ts_rs::TS)]
#[ts(export)]
pub struct BookSource {
    pub project_id: String,
    pub hash: String,
}

/// The candidate profile stored under [`MEMORY_KEY`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, ts_rs::TS)]
#[ts(export)]
pub struct SeriesProfile {
    #[serde(default)]
    pub synopsis: String,
    #[serde(default)]
    pub style_notes: Vec<String>,
    #[serde(default)]
    pub characters: Vec<SeriesCharacter>,
    /// Sources the user rejected during confirmation. Kept so a later run does not
    /// propose them again.
    #[serde(default)]
    pub rejected: Vec<String>,
    #[serde(default)]
    pub provenance: SeriesProfileProvenance,
}

#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct SeriesReconOutcome {
    pub series_id: String,
    pub model: String,
    pub books: usize,
    pub characters: usize,
    /// Books whose evidence reached the model in this run.
    pub fresh_books: usize,
    /// True when nothing changed and the stored candidate was returned untouched.
    pub from_cache: bool,
}

/// One character the user accepted from the candidate.
#[derive(Debug, Clone, Deserialize, ts_rs::TS)]
#[ts(export, optional_fields = nullable)]
pub struct ConfirmedCharacter {
    pub source: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub note: Option<String>,
}

/// Request of the structured candidate confirmation.
#[derive(Debug, Clone, Deserialize, ts_rs::TS)]
#[ts(export, optional_fields = nullable, rename = "SeriesConfirmRequest")]
pub struct ConfirmRequest {
    pub series_id: String,
    /// Final synopsis text; `None` leaves the series memory untouched.
    #[serde(default)]
    pub synopsis: Option<String>,
    /// Final style guide text; `None` leaves the series memory untouched.
    #[serde(default)]
    pub style_guide: Option<String>,
    /// Characters the user accepted, with their rendering.
    #[serde(default)]
    pub characters: Vec<ConfirmedCharacter>,
    /// Character sources the user rejected; kept so a later run skips them.
    #[serde(default)]
    pub rejected_characters: Vec<String>,
    /// Throw the rest of the candidate away (the rejections are still recorded).
    #[serde(default)]
    pub discard: bool,
}

#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export, rename = "SeriesConfirmOutcome")]
pub struct ConfirmOutcome {
    pub synopsis_updated: bool,
    pub style_guide_updated: bool,
    pub characters_accepted: usize,
    pub characters_rejected: usize,
    pub discarded: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct RawProfile {
    #[serde(default)]
    synopsis: String,
    #[serde(default)]
    style_notes: Vec<String>,
    #[serde(default)]
    characters: Vec<SeriesCharacter>,
}

/// The local evidence handed to the model: one block per book plus the glossary.
#[derive(Debug, Clone, Default)]
struct Evidence {
    books: String,
    glossary: String,
}

fn book_block(project: &Project, synopsis: &str, style_guide: &str) -> Option<String> {
    let synopsis = synopsis.trim();
    let style_guide = style_guide.trim();
    if synopsis.is_empty() && style_guide.is_empty() {
        return None;
    }
    let mut block = format!("BOOK: {}", project.name);
    if !synopsis.is_empty() {
        block.push_str(&format!("\nSYNOPSIS: {}", clamp_chars(synopsis, 1200)));
    }
    if !style_guide.is_empty() {
        block.push_str(&format!("\nSTYLE GUIDE: {}", clamp_chars(style_guide, 800)));
    }
    Some(block)
}

fn render_evidence(books: Vec<String>, glossary: Vec<String>) -> Evidence {
    let mut evidence = Evidence {
        books: String::new(),
        glossary: String::new(),
    };
    for block in books {
        if evidence.books.chars().count() + block.chars().count() > MAX_EVIDENCE_CHARS {
            break;
        }
        if !evidence.books.is_empty() {
            evidence.books.push_str("\n\n");
        }
        evidence.books.push_str(&block);
    }
    evidence.glossary = clamp_chars(&glossary.join("\n"), MAX_EVIDENCE_CHARS / 2);
    evidence
}

fn parse_profile(text: &str, provenance: SeriesProfileProvenance) -> Result<SeriesProfile> {
    let start = text
        .find('{')
        .ok_or_else(|| AppError::Invalid("the series answer contains no JSON object".into()))?;
    let end = text
        .rfind('}')
        .ok_or_else(|| AppError::Invalid("the series answer contains no JSON object".into()))?;
    let json = &text[start..=end];
    let raw: RawProfile = serde_json::from_str(json).map_err(|error| {
        AppError::Invalid(format!("the series answer is not a valid profile: {error}"))
    })?;

    let mut seen: BTreeSet<String> = BTreeSet::new();
    let characters = raw
        .characters
        .iter()
        .filter_map(|character| {
            let source = clamp_chars(&character.source, MAX_SHORT_CHARS);
            if source.is_empty() || !seen.insert(source.to_lowercase()) {
                return None;
            }
            let target = clamp_chars(&character.target, MAX_SHORT_CHARS);
            Some(SeriesCharacter {
                source,
                target: if target.is_empty() {
                    String::new()
                } else {
                    target
                },
                note: clamp_chars(&character.note, MAX_NOTE_CHARS),
            })
        })
        .take(MAX_CHARACTERS)
        .collect();

    Ok(SeriesProfile {
        synopsis: clamp_words(&raw.synopsis, MAX_SYNOPSIS_WORDS),
        style_notes: clamp_list(&raw.style_notes, MAX_STYLE_NOTES, MAX_STYLE_NOTE_CHARS),
        characters,
        rejected: Vec::new(),
        provenance,
    })
}

/// Write the shipped prompt files into the project snapshot, never overwriting an edit.
pub async fn ensure_prompt_files(dir: &Path) -> Result<()> {
    tokio::fs::create_dir_all(dir).await?;
    for (name, content) in [
        (
            "series_recon.system.md",
            DEFAULT_SERIES_RECON_SYSTEM_TEMPLATE,
        ),
        ("series_recon.user.md", DEFAULT_SERIES_RECON_USER_TEMPLATE),
        ("series_recon.schema.json", DEFAULT_SERIES_RECON_SCHEMA),
    ] {
        let path = dir.join(name);
        if !path.exists() {
            tokio::fs::write(&path, content).await?;
        }
    }
    Ok(())
}

fn read_first(dir: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(name)).ok()
}

/// Hash of a book's confirmed evidence, so an unchanged book is not re-synthesized.
fn book_hash(synopsis: &str, style_guide: &str) -> String {
    sha256_hex_str(&format!("{}\u{0}{}", synopsis.trim(), style_guide.trim()))
}

/// The previous candidate as prompt context, so an update keeps what is still valid.
fn previous_block(profile: &SeriesProfile) -> String {
    let mut out = String::new();
    if !profile.synopsis.trim().is_empty() {
        out.push_str(&format!("SYNOPSIS: {}\n", profile.synopsis));
    }
    if !profile.style_notes.is_empty() {
        out.push_str(&format!(
            "STYLE NOTES:\n- {}\n",
            profile.style_notes.join("\n- ")
        ));
    }
    if !profile.characters.is_empty() {
        out.push_str("CHARACTERS:\n");
        for character in &profile.characters {
            out.push_str(&format!("- {} => {}\n", character.source, character.target));
        }
    }
    clamp_chars(&out, MAX_EVIDENCE_CHARS)
}

/// Run the series synthesis and persist the candidate profile.
///
/// Incremental: only the books whose confirmed profile changed (or every book when nothing is
/// known yet, or `force` is set) are fed to the model, together with the previous candidate so
/// it updates rather than restarts. With nothing changed and no force, the stored candidate is
/// returned untouched and the model is not called.
pub async fn run_series_recon(
    deps: &PipelineDeps,
    job_id: Option<&str>,
    series_id: &str,
    force: bool,
) -> Result<SeriesReconOutcome> {
    let pool = &deps.pool;
    let series = repo::get_series(pool, series_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("series {series_id}")))?;
    let projects = repo::list_projects_for_series(pool, series_id).await?;
    if projects.is_empty() {
        return Err(AppError::Invalid(
            "the series has no member book: attach one first".into(),
        ));
    }
    let previous = repo::get_series_memory(pool, series_id, MEMORY_KEY)
        .await?
        .and_then(|json| serde_json::from_str::<SeriesProfile>(&json).ok());

    // Evidence: the confirmed profile of every member book, with the hash that decides
    // whether it still needs to reach the model.
    let mut all_blocks = Vec::new();
    let mut fresh_blocks = Vec::new();
    let mut sources: Vec<BookSource> = Vec::new();
    let mut book_names: Vec<String> = Vec::new();
    let previous_hashes: std::collections::HashMap<&str, &str> = previous
        .as_ref()
        .map(|profile| {
            profile
                .provenance
                .sources
                .iter()
                .map(|source| (source.project_id.as_str(), source.hash.as_str()))
                .collect()
        })
        .unwrap_or_default();
    for project in projects.iter().take(MAX_BOOKS) {
        let synopsis = repo::get_memory(pool, &project.id, "synopsis")
            .await?
            .unwrap_or_default();
        let style_guide = repo::get_memory(pool, &project.id, "style_guide")
            .await?
            .unwrap_or_default();
        let Some(block) = book_block(project, &synopsis, &style_guide) else {
            continue;
        };
        let hash = book_hash(&synopsis, &style_guide);
        let changed = previous_hashes
            .get(project.id.as_str())
            .is_none_or(|known| *known != hash.as_str());
        if changed {
            fresh_blocks.push(block.clone());
        }
        all_blocks.push(block);
        sources.push(BookSource {
            project_id: project.id.clone(),
            hash,
        });
        book_names.push(project.name.clone());
    }
    if all_blocks.is_empty() {
        return Err(AppError::Invalid(
            "no confirmed book profile: run the reconnaissance on at least one book".into(),
        ));
    }

    // The canon as the translator sees it, deduplicated across the books. A canon change
    // also invalidates a synthesis, even when no book profile moved.
    let mut glossary: BTreeSet<String> = BTreeSet::new();
    for project in &projects {
        for term in crate::pipeline::glossary::effective_terms(pool, &project.id).await? {
            glossary.insert(format!("{} => {}", term.source, term.target));
        }
    }
    let glossary_lines: Vec<String> = glossary.into_iter().take(MAX_GLOSSARY_TERMS).collect();
    let glossary_hash = sha256_hex_str(&glossary_lines.join("\n"));

    let unchanged = fresh_blocks.is_empty()
        && previous
            .as_ref()
            .is_some_and(|profile| profile.provenance.glossary_hash == glossary_hash);
    if !force && unchanged {
        let profile = previous.unwrap_or_default();
        return Ok(SeriesReconOutcome {
            series_id: series_id.to_string(),
            model: profile.provenance.model.clone(),
            books: sources.len(),
            characters: profile.characters.len(),
            fresh_books: 0,
            from_cache: true,
        });
    }
    // A forced run with nothing changed re-reads every book.
    let blocks = if fresh_blocks.is_empty() {
        all_blocks
    } else {
        fresh_blocks
    };
    let evidence = render_evidence(blocks, glossary_lines);
    let previous_profile = previous.as_ref().map(previous_block).unwrap_or_default();

    let binding = repo::role_binding_for(pool, ROLE)
        .await?
        .ok_or_else(|| AppError::Invalid("no role_binding configured for 'orchestrator'".into()))?;
    let endpoint = repo::get_endpoint(pool, &binding.endpoint_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("endpoint {}", binding.endpoint_id)))?;

    let prompts_dir = deps.prompts_dir(&projects[0].id);
    ensure_prompt_files(&prompts_dir).await?;
    let system_template = read_first(&prompts_dir, "series_recon.system.md")
        .unwrap_or_else(|| DEFAULT_SERIES_RECON_SYSTEM_TEMPLATE.to_string());
    let user_template = read_first(&prompts_dir, "series_recon.user.md")
        .unwrap_or_else(|| DEFAULT_SERIES_RECON_USER_TEMPLATE.to_string());
    let schema: Value = read_first(&prompts_dir, "series_recon.schema.json")
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| {
            serde_json::from_str(DEFAULT_SERIES_RECON_SCHEMA)
                .expect("embedded series schema is valid JSON")
        });

    let env = Environment::new();
    let system = env.render_str(
        &system_template,
        context! {
            source_language => series.source_lang.clone().unwrap_or_default(),
            target_language => series.target_lang.clone().unwrap_or_default(),
        },
    )?;
    let user = env.render_str(
        &user_template,
        context! {
            series_name => &series.name,
            previous_profile => &previous_profile,
            books => &evidence.books,
            glossary => &evidence.glossary,
            response_schema => serde_json::to_string_pretty(&schema)?,
        },
    )?;
    let prompt_hash = sha256_hex_str(&format!("{system}\n\u{0}\n{user}"));

    let fresh_books = sources
        .iter()
        .filter(|source| {
            previous_hashes
                .get(source.project_id.as_str())
                .is_none_or(|known| *known != source.hash.as_str())
        })
        .count();
    let provenance = SeriesProfileProvenance {
        generated_at: now(),
        model: binding.model.clone(),
        prompt_hash: prompt_hash.clone(),
        books: book_names,
        sources,
        glossary_hash,
    };
    let (_, profile) = run_structured_call(
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
            seed: crate::pipeline::translate::derive_seed(series_id, ROLE),
            default_max_tokens: Some(DEFAULT_MAX_TOKENS),
        },
        |text| parse_profile(text, provenance.clone()),
    )
    .await?;
    // A source the user rejected earlier is never proposed again, and the rejection is
    // carried into the new candidate.
    let rejected = previous_rejections(pool, series_id).await?;
    let mut profile = profile;
    profile
        .characters
        .retain(|character| !rejected.contains(&character.source.trim().to_lowercase()));
    profile.rejected = rejected.into_iter().collect();
    profile.rejected.sort();
    repo::set_series_memory(
        pool,
        series_id,
        MEMORY_KEY,
        &serde_json::to_string(&profile)?,
    )
    .await?;

    Ok(SeriesReconOutcome {
        series_id: series_id.to_string(),
        model: binding.model,
        books: profile.provenance.sources.len(),
        characters: profile.characters.len(),
        fresh_books,
        from_cache: false,
    })
}

/// Rejections recorded on the current candidate, lowercased for comparison.
async fn previous_rejections(pool: &sqlx::SqlitePool, series_id: &str) -> Result<BTreeSet<String>> {
    let candidate = repo::get_series_memory(pool, series_id, MEMORY_KEY)
        .await?
        .and_then(|json| serde_json::from_str::<SeriesProfile>(&json).ok())
        .unwrap_or_default();
    Ok(candidate
        .rejected
        .iter()
        .map(|source| source.trim().to_lowercase())
        .filter(|source| !source.is_empty())
        .collect())
}

/// Apply the user's decisions on the candidate: write the accepted memory, promote the
/// accepted characters into the canon and remember the rejected ones, so a later run does
/// not propose them again. Nothing is applied unless the request carries it.
pub async fn confirm(pool: &sqlx::SqlitePool, req: &ConfirmRequest) -> Result<ConfirmOutcome> {
    let series = repo::get_series(pool, &req.series_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("series {}", req.series_id)))?;

    let stored = repo::get_series_memory(pool, &req.series_id, MEMORY_KEY).await?;
    let mut candidate: SeriesProfile = stored
        .as_deref()
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or_default();

    let mut rejected: BTreeSet<String> = candidate
        .rejected
        .iter()
        .map(|source| source.trim().to_lowercase())
        .filter(|source| !source.is_empty())
        .collect();
    let mut rejected_count = 0;
    for source in &req.rejected_characters {
        let key = source.trim().to_lowercase();
        if !key.is_empty() && rejected.insert(key) {
            rejected_count += 1;
        }
    }

    // Discard keeps the rejections (the only durable decision) and drops the rest.
    if req.discard {
        if rejected.is_empty() {
            repo::delete_series_memory(pool, &req.series_id, MEMORY_KEY).await?;
        } else {
            let kept = SeriesProfile {
                rejected: rejected.into_iter().collect(),
                ..SeriesProfile::default()
            };
            repo::set_series_memory(
                pool,
                &req.series_id,
                MEMORY_KEY,
                &serde_json::to_string(&kept)?,
            )
            .await?;
        }
        return Ok(ConfirmOutcome {
            synopsis_updated: false,
            style_guide_updated: false,
            characters_accepted: 0,
            characters_rejected: rejected_count,
            discarded: true,
        });
    }

    let mut synopsis_updated = false;
    if let Some(synopsis) = req
        .synopsis
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        repo::set_series_memory(pool, &req.series_id, "synopsis", synopsis).await?;
        synopsis_updated = true;
    }
    let mut style_guide_updated = false;
    if let Some(style_guide) = req
        .style_guide
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        repo::set_series_memory(pool, &req.series_id, "style_guide", style_guide).await?;
        style_guide_updated = true;
    }

    let mut accepted: BTreeSet<String> = BTreeSet::new();
    let mut characters_accepted = 0;
    for character in &req.characters {
        let source = character.source.trim();
        let target = character.target.trim();
        if source.is_empty() || target.is_empty() {
            continue;
        }
        accepted.insert(source.to_lowercase());
        repo::upsert_series_term(
            pool,
            &crate::db::models::SeriesGlossaryTerm {
                id: new_id(),
                series_id: series.id.clone(),
                source_lang: series.source_lang.clone().or_else(|| Some(String::new())),
                target_lang: series.target_lang.clone().or_else(|| Some(String::new())),
                source: source.to_string(),
                target: target.to_string(),
                note: character
                    .note
                    .as_deref()
                    .map(str::trim)
                    .filter(|note| !note.is_empty())
                    .map(str::to_string),
                kind: "proper_noun".to_string(),
                origin: "proposed".to_string(),
                revision: 1,
                status: "approved".to_string(),
            },
        )
        .await?;
        if let Err(error) =
            super::glossary::flag_series_conflicts(pool, &series.id, source, target).await
        {
            tracing::warn!(%error, "could not flag series conflicts after confirming");
        }
        characters_accepted += 1;
    }
    // Accepting a character that had been rejected before clears the rejection.
    for source in &accepted {
        rejected.remove(source);
    }

    let remaining = candidate.characters.iter().any(|character| {
        let key = character.source.trim().to_lowercase();
        !accepted.contains(&key) && !rejected.contains(&key)
    });
    if stored.is_some() || !rejected.is_empty() || remaining {
        candidate.characters.retain(|character| {
            let key = character.source.trim().to_lowercase();
            !accepted.contains(&key) && !rejected.contains(&key)
        });
        candidate.rejected = rejected.into_iter().collect();
        candidate.rejected.sort();
        repo::set_series_memory(
            pool,
            &req.series_id,
            MEMORY_KEY,
            &serde_json::to_string(&candidate)?,
        )
        .await?;
    } else {
        repo::delete_series_memory(pool, &req.series_id, MEMORY_KEY).await?;
    }

    Ok(ConfirmOutcome {
        synopsis_updated,
        style_guide_updated,
        characters_accepted,
        characters_rejected: rejected_count,
        discarded: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn series_profile_is_clamped_and_deduplicated() {
        let raw = r#"{"synopsis":"  A saga.  ","style_notes":["note","note"],"characters":[
            {"source":"Keeper","target":"Custode","note":"n"},
            {"source":"keeper","target":"altra","note":"dup"},
            {"source":"","target":"x"},
            {"source":"Ship","target":""}
        ]}"#;
        let profile = parse_profile(raw, SeriesProfileProvenance::default()).expect("profile");
        assert_eq!(profile.synopsis, "A saga.");
        // `clamp_list` deduplicates the notes.
        assert_eq!(profile.style_notes.len(), 1);
        assert_eq!(profile.characters.len(), 2);
        assert_eq!(profile.characters[0].source, "Keeper");
        assert_eq!(profile.characters[1].target, "");
    }

    #[test]
    fn embedded_series_prompts_match_the_repository_files() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let file = std::fs::read_to_string(root.join("prompts/series_recon.md"))
            .expect("read prompts/series_recon.md");
        let (system, user) = file
            .split_once("-->\n")
            .expect("series header")
            .1
            .split_once("\n---USER---\n")
            .expect("the file must contain the ---USER--- marker");
        assert_eq!(system, DEFAULT_SERIES_RECON_SYSTEM_TEMPLATE);
        assert_eq!(user, DEFAULT_SERIES_RECON_USER_TEMPLATE);

        let file: Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("prompts/series_recon.schema.json"))
                .expect("read prompts/series_recon.schema.json"),
        )
        .expect("schema is valid JSON");
        let embedded: Value =
            serde_json::from_str(DEFAULT_SERIES_RECON_SCHEMA).expect("embedded schema is JSON");
        assert_eq!(file, embedded);
    }

    async fn seed(pool: &sqlx::SqlitePool) -> crate::db::models::Series {
        let timestamp = now();
        let series = crate::db::models::Series {
            id: "s1".to_string(),
            name: "The Saga".to_string(),
            source_lang: Some("English".to_string()),
            target_lang: Some("Italian".to_string()),
            settings_json: "{}".to_string(),
            created_at: timestamp.clone(),
            updated_at: timestamp,
        };
        repo::upsert_series(pool, &series).await.expect("series");
        series
    }

    async fn seed_candidate(pool: &sqlx::SqlitePool, series_id: &str, profile: &SeriesProfile) {
        repo::set_series_memory(
            pool,
            series_id,
            MEMORY_KEY,
            &serde_json::to_string(profile).unwrap(),
        )
        .await
        .expect("candidate");
    }

    #[tokio::test]
    async fn confirm_applies_accepted_fields_and_promotes_characters() {
        let (pool, _dir) = crate::db::connect_temp_file().await.expect("pool");
        let series = seed(&pool).await;
        seed_candidate(
            &pool,
            &series.id,
            &SeriesProfile {
                synopsis: "candidate".to_string(),
                style_notes: vec!["note".to_string()],
                characters: vec![
                    SeriesCharacter {
                        source: "Keeper".to_string(),
                        target: "Custode".to_string(),
                        note: "protagonist".to_string(),
                    },
                    SeriesCharacter {
                        source: "Ship".to_string(),
                        target: "Nave".to_string(),
                        note: String::new(),
                    },
                ],
                rejected: Vec::new(),
                provenance: SeriesProfileProvenance::default(),
            },
        )
        .await;

        let outcome = confirm(
            &pool,
            &ConfirmRequest {
                series_id: series.id.clone(),
                synopsis: Some("The saga of the light.".to_string()),
                style_guide: Some("Formal register.".to_string()),
                characters: vec![ConfirmedCharacter {
                    source: "Keeper".to_string(),
                    target: "Custode".to_string(),
                    note: Some("protagonist".to_string()),
                }],
                rejected_characters: vec!["Ship".to_string()],
                discard: false,
            },
        )
        .await
        .expect("confirm");
        assert!(outcome.synopsis_updated);
        assert!(outcome.style_guide_updated);
        assert_eq!(outcome.characters_accepted, 1);
        assert_eq!(outcome.characters_rejected, 1);

        assert_eq!(
            repo::get_series_memory(&pool, &series.id, "synopsis")
                .await
                .expect("synopsis")
                .as_deref(),
            Some("The saga of the light.")
        );
        let terms = repo::list_series_terms(&pool, &series.id)
            .await
            .expect("terms");
        assert_eq!(terms.len(), 1);
        assert_eq!(terms[0].source, "Keeper");
        assert_eq!(terms[0].status, "approved");
        assert_eq!(terms[0].origin, "proposed");

        // The candidate keeps only the undecided part and remembers the rejection.
        let stored = repo::get_series_memory(&pool, &series.id, MEMORY_KEY)
            .await
            .expect("candidate")
            .expect("still there");
        let candidate: SeriesProfile = serde_json::from_str(&stored).expect("parse");
        assert!(candidate.characters.is_empty());
        assert_eq!(candidate.rejected, vec!["ship".to_string()]);
    }

    #[tokio::test]
    async fn discarding_keeps_only_the_rejections() {
        let (pool, _dir) = crate::db::connect_temp_file().await.expect("pool");
        let series = seed(&pool).await;
        seed_candidate(
            &pool,
            &series.id,
            &SeriesProfile {
                synopsis: "candidate".to_string(),
                style_notes: vec!["note".to_string()],
                characters: vec![SeriesCharacter {
                    source: "Ship".to_string(),
                    target: "Nave".to_string(),
                    note: String::new(),
                }],
                rejected: Vec::new(),
                provenance: SeriesProfileProvenance::default(),
            },
        )
        .await;

        let outcome = confirm(
            &pool,
            &ConfirmRequest {
                series_id: series.id.clone(),
                synopsis: Some("ignored".to_string()),
                style_guide: None,
                characters: Vec::new(),
                rejected_characters: vec!["Ship".to_string()],
                discard: true,
            },
        )
        .await
        .expect("discard");
        assert!(outcome.discarded);
        assert!(!outcome.synopsis_updated);
        assert!(repo::get_series_memory(&pool, &series.id, "synopsis")
            .await
            .expect("synopsis")
            .is_none());
        let stored = repo::get_series_memory(&pool, &series.id, MEMORY_KEY)
            .await
            .expect("candidate")
            .expect("kept for the rejection");
        let candidate: SeriesProfile = serde_json::from_str(&stored).expect("parse");
        assert!(candidate.characters.is_empty());
        assert!(candidate.synopsis.is_empty());
        assert_eq!(candidate.rejected, vec!["ship".to_string()]);
    }
}
