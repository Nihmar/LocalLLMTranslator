//! Book reconnaissance (PLAN.md section 9.4): a candidate book profile built
//! from local evidence only.
//!
//! The call runs on the `orchestrator` role behind a JSON schema and the answer
//! is stored as a **candidate** under `project_memory['book_profile']`. Nothing
//! reaches the translator prompts until the user confirms the fields: [`confirm`]
//! writes `style_guide` and `synopsis` where the context builder already reads
//! them, the rest as `book_meta`, and the proper nouns as glossary terms.
//!
//! Evidence is local by construction: the extractor metadata, the opening
//! paragraphs of the chapters, and text the user pasted. The app never fetches
//! anything.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use minijinja::{context, Environment};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::SqlitePool;

use super::chat_call::{run_structured_call, ChatCall};
use super::PipelineDeps;
use crate::db::models::{Block, Chapter, GlossaryTerm};
use crate::db::{new_id, now, repo};
use crate::error::{AppError, Result};
use crate::util::{clamp_chars, clamp_list, clamp_words, sha256_hex_str};

/// Role binding used for the reconnaissance call.
pub const ROLE: &str = "orchestrator";
/// Job kind that runs the reconnaissance.
pub const JOB_KIND: &str = "book_recon";
/// Memory key holding the candidate profile.
pub const MEMORY_KEY: &str = "book_profile";
/// Memory key holding the confirmed non-style-guide fields.
pub const META_KEY: &str = "book_meta";
pub const STYLE_GUIDE_KEY: &str = "style_guide";
/// Project memory key of the dialogue convention: `keep` (the source's dash, the
/// default) or `quotes` (target-language quotation marks). Series memory works as a
/// fallback, like the style guide.
pub const DIALOGUE_STYLE_KEY: &str = "dialogue_style";

/// The dialogue conventions a project can choose, the default first.
pub const DIALOGUE_STYLES: [&str; 2] = ["keep", "quotes"];

/// Whether a stored dialogue style asks for quotation marks; anything else keeps
/// the source's dash.
pub fn wants_dialogue_quotes(style: &str) -> bool {
    style.trim() == "quotes"
}
pub const SYNOPSIS_KEY: &str = "synopsis";

/// `response_format.json_schema.name` the model sees.
const SCHEMA_NAME: &str = "book_profile";

// Caps. PLAN.md section 9.4: the profile is paid on every chunk, so it cannot
// grow unbound. The schema states the same limits; these clamp a non-compliant
// answer before it is persisted.
const MAX_SOURCE_LANG_CHARS: usize = 60;
const MAX_SHORT_CHARS: usize = 120;
const MAX_LONG_CHARS: usize = 300;
const MAX_SYNOPSIS_WORDS: usize = 120;
const MAX_STYLE_GUIDE_WORDS: usize = 200;
const MAX_STYLE_NOTES: usize = 8;
const MAX_STYLE_NOTE_CHARS: usize = 200;
const MAX_THEMES: usize = 8;
const MAX_THEME_CHARS: usize = 80;
const MAX_PROPER_NOUNS: usize = 12;
const MAX_TERM_CHARS: usize = 120;
const MAX_NOTE_CHARS: usize = 200;
const DEFAULT_MAX_TOKENS: u32 = 1200;

/// Local evidence handed to the model.
const MAX_PASTED_CHARS: usize = 12_000;
const MAX_EXCERPT_CHARS: usize = 12_000;
const PER_EXCERPT_CHARS: usize = 700;
const EXCERPTS_PER_CHAPTER: usize = 2;
const MAX_EXCERPTS: usize = 24;
const MAX_METADATA_CHARS: usize = 2_000;
/// Block kinds worth quoting: prose, not code, tables or figures.
const EXCERPT_KINDS: [&str; 3] = ["para", "blockquote", "list"];

/// Fallback templates, byte-identical to `prompts/analyze_book.md` (there is a
/// unit test asserting that) so a project snapshot from before M3 still runs.
pub const DEFAULT_ANALYZE_SYSTEM_TEMPLATE: &str = r#"You are the book analyst of a {{ source_language }} → {{ target_language }} translation project.
You receive local evidence only: the metadata the extractor produced, the opening paragraphs of
the book and, when present, material the user pasted. You never fetch anything and you never
invent facts the evidence cannot support.

Produce a CANDIDATE profile that the user will review field by field. For every field state the
basis: "from_text" when the evidence shows it, "metadata" when it comes from the extractor
metadata, "inferred" when you are reasoning beyond the evidence.

Reply with JSON only, no commentary, no code fences, matching this shape:
{"source_language": "...", "genre": "...", "audience": "...", "era": "...",
 "narrative_voice": "...", "register": "...",
 "style_notes": ["short, actionable translation instructions"],
 "themes": ["..."], "synopsis": "3-5 sentences",
 "proper_nouns": [{"source": "...", "kind": "proper_noun|do_not_translate", "note": "..."}],
 "field_basis": {"genre": "from_text|metadata|inferred", "synopsis": "from_text"}}

RULES
1. Write genre, audience, era, narrative_voice, register, style_notes, themes and the synopsis in {{ target_language }}.
2. Keep the synopsis under 120 words: it is injected into every translation prompt, so it is paid on every chunk.
3. At most 8 style_notes, at most 20 words each, concrete and actionable: register, forms of address, sentence rhythm, recurring constructions. No generic advice about "translating well".
4. At most 8 themes, 2-4 words each.
5. At most 12 proper_nouns: names that recur or matter. Use kind="do_not_translate" for names that must stay verbatim (brands, invented words, place names with no established rendering) and kind="proper_noun" otherwise. One short note each.
6. When the evidence is too thin for a field, leave the value empty and mark the basis "inferred" instead of guessing. An empty field is a valid answer."#;

pub const DEFAULT_ANALYZE_USER_TEMPLATE: &str = r#"BOOK METADATA (from the extractor; may be empty):
{{ metadata }}

EXCERPTS (incipit and opening paragraphs of the chapters):
{{ excerpts }}

USER-PASTED MATERIAL (optional):
{{ pasted_text }}
"#;

pub const DEFAULT_ANALYZE_SCHEMA: &str = r##"{"$comment":"Book reconnaissance schema (PLAN.md section 9.4). The length caps are the ones the model is asked to respect; the control plane clamps every value again before persisting it, and clamps the synopsis to 120 words and the assembled style guide to 200 words, because JSON Schema cannot express word counts.","type":"object","properties":{"source_language":{"type":"string","maxLength":60},"genre":{"type":"string","maxLength":120},"audience":{"type":"string","maxLength":120},"era":{"type":"string","maxLength":120},"narrative_voice":{"type":"string","maxLength":300},"register":{"type":"string","maxLength":300},"style_notes":{"type":"array","maxItems":8,"items":{"type":"string","maxLength":240}},"themes":{"type":"array","maxItems":8,"items":{"type":"string","maxLength":80}},"synopsis":{"type":"string","maxLength":800},"proper_nouns":{"type":"array","maxItems":12,"items":{"type":"object","properties":{"source":{"type":"string","maxLength":120},"kind":{"enum":["proper_noun","do_not_translate"]},"note":{"type":"string","maxLength":200}},"required":["source","kind"]}},"field_basis":{"type":"object","properties":{"source_language":{"enum":["from_text","metadata","inferred"]},"genre":{"enum":["from_text","metadata","inferred"]},"audience":{"enum":["from_text","metadata","inferred"]},"era":{"enum":["from_text","metadata","inferred"]},"narrative_voice":{"enum":["from_text","metadata","inferred"]},"register":{"enum":["from_text","metadata","inferred"]},"style_notes":{"enum":["from_text","metadata","inferred"]},"themes":{"enum":["from_text","metadata","inferred"]},"synopsis":{"enum":["from_text","metadata","inferred"]}}}},"required":["source_language","genre","audience","era","narrative_voice","register","style_notes","themes","synopsis","proper_nouns","field_basis"]}"##;

/// Keys a user may confirm; unknown keys in a request are ignored.
const PROFILE_FIELDS: [&str; 9] = [
    "source_language",
    "genre",
    "audience",
    "era",
    "narrative_voice",
    "register",
    "style_notes",
    "themes",
    "synopsis",
];

// ---------------------------------------------------------------------------
// Profile types
// ---------------------------------------------------------------------------

/// One profile value with its provenance. `basis` is one of `from_text`,
/// `metadata`, `inferred` (PLAN.md section 9.4): an `inferred` field is shown as
/// such and never silently becomes a fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProfileField<T> {
    pub value: T,
    pub basis: String,
}

/// A name the profile proposes for the glossary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProperNoun {
    pub source: String,
    pub kind: String,
    #[serde(default)]
    pub note: String,
}

/// Where the profile came from; travels with the value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProfileProvenance {
    pub generated_at: String,
    pub model: String,
    pub prompt_hash: String,
    pub excerpt_blocks: usize,
    pub metadata: bool,
    pub pasted_chars: usize,
}

/// The candidate profile. Stored as JSON under [`MEMORY_KEY`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BookProfile {
    pub source_language: ProfileField<String>,
    pub genre: ProfileField<String>,
    pub audience: ProfileField<String>,
    pub era: ProfileField<String>,
    pub narrative_voice: ProfileField<String>,
    pub register: ProfileField<String>,
    pub style_notes: ProfileField<Vec<String>>,
    pub themes: ProfileField<Vec<String>>,
    pub synopsis: ProfileField<String>,
    pub proper_nouns: Vec<ProperNoun>,
    #[serde(default)]
    pub provenance: ProfileProvenance,
}

/// What `recon_get` returns: candidate + already-confirmed values + glossary.
#[derive(Debug, Clone, Serialize)]
pub struct ReconSnapshot {
    pub project_id: String,
    pub profile: Option<BookProfile>,
    pub style_guide: String,
    pub synopsis: String,
    pub book_meta: Option<Value>,
    pub glossary: Vec<GlossaryTerm>,
    /// Style-note candidates proposed by the summarizer; the user decides which
    /// ones enter the style guide.
    pub style_notes: Vec<String>,
    /// `keep` or `quotes` (see [`DIALOGUE_STYLE_KEY`]).
    pub dialogue_style: String,
    pub orchestrator_bound: bool,
    /// Id of a pending/leased/running `book_recon` job, when there is one.
    pub running_job: Option<String>,
    /// Last failure of a `book_recon` job, when there is one.
    pub last_error: Option<String>,
}

/// Request of `recon_confirm`.
#[derive(Debug, Clone, Deserialize)]
pub struct ConfirmRequest {
    pub project_id: String,
    pub profile: BookProfile,
    /// Profile keys the user accepted; only those are written.
    #[serde(default)]
    pub confirmed_fields: Vec<String>,
    /// Style guide text assembled and edited in the UI (clamped here too).
    #[serde(default)]
    pub style_guide: String,
    /// Proper nouns the user accepted, with an optional target.
    #[serde(default)]
    pub proper_nouns: Vec<ConfirmedTerm>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConfirmedTerm {
    pub source: String,
    #[serde(default)]
    pub target: Option<String>,
    pub kind: String,
    #[serde(default)]
    pub note: Option<String>,
}

/// Result of one reconnaissance run.
#[derive(Debug, Clone, Serialize)]
pub struct ReconOutcome {
    pub project_id: String,
    pub model: String,
    pub proper_nouns: usize,
    pub excerpt_blocks: usize,
}

/// The raw model answer, before clamping.
#[derive(Debug, Clone, Deserialize, Default)]
struct RawProfile {
    #[serde(default)]
    source_language: String,
    #[serde(default)]
    genre: String,
    #[serde(default)]
    audience: String,
    #[serde(default)]
    era: String,
    #[serde(default)]
    narrative_voice: String,
    #[serde(default)]
    register: String,
    #[serde(default)]
    style_notes: Vec<String>,
    #[serde(default)]
    themes: Vec<String>,
    #[serde(default)]
    synopsis: String,
    #[serde(default)]
    proper_nouns: Vec<ProperNoun>,
    #[serde(default)]
    field_basis: BTreeMap<String, String>,
}

impl RawProfile {
    fn sanitize(&self, provenance: ProfileProvenance) -> BookProfile {
        BookProfile {
            source_language: ProfileField {
                value: clamp_chars(&self.source_language, MAX_SOURCE_LANG_CHARS),
                basis: basis_for(&self.field_basis, "source_language"),
            },
            genre: ProfileField {
                value: clamp_chars(&self.genre, MAX_SHORT_CHARS),
                basis: basis_for(&self.field_basis, "genre"),
            },
            audience: ProfileField {
                value: clamp_chars(&self.audience, MAX_SHORT_CHARS),
                basis: basis_for(&self.field_basis, "audience"),
            },
            era: ProfileField {
                value: clamp_chars(&self.era, MAX_SHORT_CHARS),
                basis: basis_for(&self.field_basis, "era"),
            },
            narrative_voice: ProfileField {
                value: clamp_chars(&self.narrative_voice, MAX_LONG_CHARS),
                basis: basis_for(&self.field_basis, "narrative_voice"),
            },
            register: ProfileField {
                value: clamp_chars(&self.register, MAX_LONG_CHARS),
                basis: basis_for(&self.field_basis, "register"),
            },
            style_notes: ProfileField {
                value: clamp_list(&self.style_notes, MAX_STYLE_NOTES, MAX_STYLE_NOTE_CHARS),
                basis: basis_for(&self.field_basis, "style_notes"),
            },
            themes: ProfileField {
                value: clamp_list(&self.themes, MAX_THEMES, MAX_THEME_CHARS),
                basis: basis_for(&self.field_basis, "themes"),
            },
            synopsis: ProfileField {
                value: clamp_words(&self.synopsis, MAX_SYNOPSIS_WORDS),
                basis: basis_for(&self.field_basis, "synopsis"),
            },
            proper_nouns: sanitize_terms(&self.proper_nouns),
            provenance,
        }
    }
}

// ---------------------------------------------------------------------------
// Local evidence
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct Evidence {
    metadata: String,
    excerpts: String,
    blocks: usize,
}

#[derive(Debug, Clone, Default)]
struct Section {
    title: String,
    excerpts: Vec<String>,
}

/// First paragraphs of the book, grouped by chapter. Pure, so it is unit-tested
/// without a database.
fn excerpts_from(blocks: &[Block], chapters: &[Chapter]) -> Evidence {
    let titles: HashMap<&str, &str> = chapters
        .iter()
        .map(|chapter| (chapter.id.as_str(), chapter.title.as_str()))
        .collect();
    let mut used: HashMap<String, usize> = HashMap::new();
    let mut sections: Vec<(String, Section)> = Vec::new();
    let mut total_chars = 0usize;
    let mut count = 0usize;

    for block in blocks {
        if count >= MAX_EXCERPTS {
            break;
        }
        if !block.translatable || !EXCERPT_KINDS.contains(&block.kind.as_str()) {
            continue;
        }
        let text = block.source_text.trim();
        if text.is_empty() {
            continue;
        }
        let key = block.chapter_id.clone().unwrap_or_default();
        let taken = used.entry(key.clone()).or_insert(0);
        if *taken >= EXCERPTS_PER_CHAPTER {
            continue;
        }
        let excerpt = clamp_chars(text, PER_EXCERPT_CHARS);
        let size = excerpt.chars().count();
        if total_chars + size > MAX_EXCERPT_CHARS {
            break;
        }
        *taken += 1;
        total_chars += size;
        count += 1;

        let title = if key.is_empty() {
            "Incipit".to_string()
        } else {
            titles
                .get(key.as_str())
                .map(|title| (*title).to_string())
                .unwrap_or_else(|| "Section".to_string())
        };
        match sections
            .iter()
            .position(|(section_key, _)| section_key.as_str() == key.as_str())
        {
            Some(index) => sections[index].1.excerpts.push(excerpt),
            None => sections.push((
                key,
                Section {
                    title,
                    excerpts: vec![excerpt],
                },
            )),
        }
    }

    let excerpts = sections
        .iter()
        .map(|(_, section)| format!("## {}\n{}", section.title, section.excerpts.join("\n\n")))
        .collect::<Vec<_>>()
        .join("\n\n");

    Evidence {
        metadata: String::new(),
        excerpts,
        blocks: count,
    }
}

/// Extractor metadata as compact JSON, capped.
fn metadata_text(front_matter_json: &str) -> String {
    let parsed: Option<Value> = serde_json::from_str(front_matter_json).ok();
    match parsed {
        Some(Value::Object(map)) if !map.is_empty() => {
            let pretty = serde_json::to_string_pretty(&Value::Object(map)).unwrap_or_default();
            clamp_chars(&pretty, MAX_METADATA_CHARS)
        }
        _ => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Clamping helpers
// ---------------------------------------------------------------------------

fn sanitize_terms(terms: &[ProperNoun]) -> Vec<ProperNoun> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::new();
    for term in terms {
        let source = clamp_chars(&term.source, MAX_TERM_CHARS);
        if source.is_empty() || !seen.insert(source.to_lowercase()) {
            continue;
        }
        out.push(ProperNoun {
            source,
            kind: normalize_kind(&term.kind),
            note: clamp_chars(&term.note, MAX_NOTE_CHARS),
        });
        if out.len() >= MAX_PROPER_NOUNS {
            break;
        }
    }
    out
}

fn normalize_kind(raw: &str) -> String {
    if raw.trim().eq_ignore_ascii_case("do_not_translate") {
        "do_not_translate".to_string()
    } else {
        "proper_noun".to_string()
    }
}

fn basis_for(basis: &BTreeMap<String, String>, key: &str) -> String {
    match basis.get(key).map(String::as_str) {
        Some("from_text") => "from_text",
        Some("metadata") => "metadata",
        _ => "inferred",
    }
    .to_string()
}

/// Pull the JSON object out of a reply that may carry code fences or prose.
fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| &text[start..=end])
}

fn parse_profile(text: &str, provenance: ProfileProvenance) -> Result<BookProfile> {
    let json = extract_json_object(text).ok_or_else(|| {
        AppError::Invalid("the orchestrator answer contains no JSON object".into())
    })?;
    let raw: RawProfile = serde_json::from_str(json).map_err(|error| {
        AppError::Invalid(format!(
            "the orchestrator answer is not a valid book profile: {error}"
        ))
    })?;
    Ok(raw.sanitize(provenance))
}

// ---------------------------------------------------------------------------
// Prompt files
// ---------------------------------------------------------------------------

/// Write the shipped reconnaissance prompt files into a project snapshot, but
/// never overwrite an edited one.
pub async fn ensure_prompt_files(dir: &Path) -> Result<()> {
    tokio::fs::create_dir_all(dir).await?;
    for (name, content) in [
        ("analyze_book.system.md", DEFAULT_ANALYZE_SYSTEM_TEMPLATE),
        ("analyze_book.user.md", DEFAULT_ANALYZE_USER_TEMPLATE),
        ("analyze_book.schema.json", DEFAULT_ANALYZE_SCHEMA),
    ] {
        let path = dir.join(name);
        if !path.exists() {
            tokio::fs::write(&path, content).await?;
        }
    }
    Ok(())
}

fn read_first(dir: &Path, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| std::fs::read_to_string(dir.join(name)).ok())
}

fn load_templates(dir: &Path) -> (String, String) {
    let system = read_first(dir, &["analyze_book.system.md"])
        .unwrap_or_else(|| DEFAULT_ANALYZE_SYSTEM_TEMPLATE.to_string());
    let user = read_first(dir, &["analyze_book.user.md"])
        .unwrap_or_else(|| DEFAULT_ANALYZE_USER_TEMPLATE.to_string());
    (system, user)
}

fn load_schema(dir: &Path) -> Value {
    read_first(dir, &["analyze_book.schema.json"])
        .and_then(|text| serde_json::from_str(&text).ok())
        // The embedded schema is a compile-time constant: parsing cannot fail.
        .unwrap_or_else(|| {
            serde_json::from_str(DEFAULT_ANALYZE_SCHEMA).expect("embedded schema is valid JSON")
        })
}

// ---------------------------------------------------------------------------
// Run
// ---------------------------------------------------------------------------

/// Run the reconnaissance for a project and store the candidate profile.
pub async fn run_recon(
    deps: &PipelineDeps,
    job_id: Option<&str>,
    project_id: &str,
    pasted_text: Option<&str>,
) -> Result<ReconOutcome> {
    let pool = &deps.pool;
    let project = repo::get_project(pool, project_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("project {project_id}")))?;
    let document = repo::get_document_for_project(pool, project_id)
        .await?
        .ok_or_else(|| {
            AppError::Invalid("the project has no ingested document: run ingestion first".into())
        })?;

    let binding = repo::role_binding_for(pool, ROLE).await?.ok_or_else(|| {
        AppError::Invalid(
            "no role_binding configured for 'orchestrator': bind a model to run the reconnaissance"
                .into(),
        )
    })?;
    let endpoint = repo::get_endpoint(pool, &binding.endpoint_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("endpoint {}", binding.endpoint_id)))?;

    let chapters = repo::list_chapters(pool, &document.id).await?;
    let blocks = repo::list_blocks(pool, &document.id).await?;
    let mut evidence = excerpts_from(&blocks, &chapters);
    evidence.metadata = metadata_text(&document.front_matter_json);
    let pasted = clamp_chars(pasted_text.unwrap_or_default(), MAX_PASTED_CHARS);

    let prompts_dir = deps.prompts_dir(project_id);
    ensure_prompt_files(&prompts_dir).await?;
    let (system_template, user_template) = load_templates(&prompts_dir);
    let schema = load_schema(&prompts_dir);

    let env = Environment::new();
    let source_language = project.source_lang.clone().unwrap_or_default();
    let system = env.render_str(
        &system_template,
        context! {
            source_language => &source_language,
            target_language => &project.target_lang,
        },
    )?;
    let user = env.render_str(
        &user_template,
        context! {
            metadata => &evidence.metadata,
            excerpts => &evidence.excerpts,
            pasted_text => &pasted,
        },
    )?;
    let prompt_hash = sha256_hex_str(&format!("{system}\n\u{0}\n{user}"));

    let provenance = ProfileProvenance {
        generated_at: now(),
        model: binding.model.clone(),
        prompt_hash: prompt_hash.clone(),
        excerpt_blocks: evidence.blocks,
        metadata: !evidence.metadata.is_empty(),
        pasted_chars: pasted.chars().count(),
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
            seed: crate::pipeline::translate::derive_seed(&project.id, ROLE),
            default_max_tokens: Some(DEFAULT_MAX_TOKENS),
        },
        |text| parse_profile(text, provenance.clone()),
    )
    .await?;
    repo::set_memory(
        pool,
        project_id,
        MEMORY_KEY,
        &serde_json::to_string(&profile)?,
    )
    .await?;

    Ok(ReconOutcome {
        project_id: project_id.to_string(),
        model: binding.model,
        proper_nouns: profile.proper_nouns.len(),
        excerpt_blocks: evidence.blocks,
    })
}

// ---------------------------------------------------------------------------
// Snapshot and confirmation
// ---------------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
struct LatestJob {
    id: String,
    state: String,
    last_error: Option<String>,
}

/// Read the candidate, the confirmed values and the glossary.
pub async fn snapshot(pool: &SqlitePool, project_id: &str) -> Result<ReconSnapshot> {
    repo::get_project(pool, project_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("project {project_id}")))?;
    let profile = repo::get_memory(pool, project_id, MEMORY_KEY)
        .await?
        .and_then(|json| serde_json::from_str::<BookProfile>(&json).ok());
    let style_guide = repo::get_memory(pool, project_id, STYLE_GUIDE_KEY)
        .await?
        .unwrap_or_default();
    let synopsis = repo::get_memory(pool, project_id, SYNOPSIS_KEY)
        .await?
        .unwrap_or_default();
    let book_meta = repo::get_memory(pool, project_id, META_KEY)
        .await?
        .and_then(|json| serde_json::from_str::<Value>(&json).ok());
    let glossary = repo::list_glossary_terms(pool, project_id).await?;
    let style_notes = crate::pipeline::summarize::stored_style_notes(pool, project_id).await?;
    let dialogue_style = repo::get_memory(pool, project_id, DIALOGUE_STYLE_KEY)
        .await?
        .filter(|style| DIALOGUE_STYLES.contains(&style.as_str()))
        .unwrap_or_else(|| DIALOGUE_STYLES[0].to_string());
    let orchestrator_bound = repo::role_binding_for(pool, ROLE).await?.is_some();

    let latest: Option<LatestJob> = sqlx::query_as(
        "SELECT id, state, last_error FROM job WHERE project_id = ?1 AND kind = ?2 \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(project_id)
    .bind(JOB_KIND)
    .fetch_optional(pool)
    .await?;
    let (running_job, last_error) = match latest {
        Some(job) if matches!(job.state.as_str(), "pending" | "leased" | "running") => {
            (Some(job.id), None)
        }
        Some(job) => (None, job.last_error),
        None => (None, None),
    };

    Ok(ReconSnapshot {
        project_id: project_id.to_string(),
        profile,
        style_guide,
        synopsis,
        book_meta,
        glossary,
        style_notes,
        dialogue_style,
        orchestrator_bound,
        running_job,
        last_error,
    })
}

/// Persist the fields the user confirmed: `style_guide`/`synopsis` where the
/// context builder reads them, the rest as `book_meta`, proper nouns as
/// approved glossary terms.
pub async fn confirm(pool: &SqlitePool, req: &ConfirmRequest) -> Result<ReconSnapshot> {
    let project = repo::get_project(pool, &req.project_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("project {}", req.project_id)))?;

    let style_guide = clamp_words(&req.style_guide, MAX_STYLE_GUIDE_WORDS);
    repo::set_memory(pool, &req.project_id, STYLE_GUIDE_KEY, &style_guide).await?;

    let mut fields = serde_json::Map::new();
    for key in &req.confirmed_fields {
        if !PROFILE_FIELDS.contains(&key.as_str()) {
            continue;
        }
        let value = match key.as_str() {
            "source_language" => serde_json::to_value(&req.profile.source_language)?,
            "genre" => serde_json::to_value(&req.profile.genre)?,
            "audience" => serde_json::to_value(&req.profile.audience)?,
            "era" => serde_json::to_value(&req.profile.era)?,
            "narrative_voice" => serde_json::to_value(&req.profile.narrative_voice)?,
            "register" => serde_json::to_value(&req.profile.register)?,
            "style_notes" => serde_json::to_value(&req.profile.style_notes)?,
            "themes" => serde_json::to_value(&req.profile.themes)?,
            "synopsis" => serde_json::to_value(&req.profile.synopsis)?,
            _ => continue,
        };
        fields.insert(key.clone(), value);
    }

    let confirmed_fields: BTreeSet<&str> = req
        .confirmed_fields
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();

    if confirmed_fields.contains(SYNOPSIS_KEY) {
        let synopsis = clamp_words(&req.profile.synopsis.value, MAX_SYNOPSIS_WORDS);
        repo::set_memory(pool, &req.project_id, SYNOPSIS_KEY, &synopsis).await?;
    }

    let confirmed_source_language = confirmed_fields
        .contains("source_language")
        .then(|| clamp_chars(&req.profile.source_language.value, MAX_SOURCE_LANG_CHARS))
        .filter(|value| !value.is_empty());
    if let Some(language) = &confirmed_source_language {
        sqlx::query("UPDATE project SET source_lang = ?2, updated_at = ?3 WHERE id = ?1")
            .bind(&req.project_id)
            .bind(language)
            .bind(now())
            .execute(pool)
            .await?;
    }
    let glossary_source_lang = confirmed_source_language
        .clone()
        .or_else(|| project.source_lang.clone())
        .filter(|value| !value.trim().is_empty());

    let existing = repo::list_glossary_terms(pool, &req.project_id).await?;
    let mut written_terms = Vec::new();
    for term in &req.proper_nouns {
        let source = clamp_chars(&term.source, MAX_TERM_CHARS);
        if source.is_empty() {
            continue;
        }
        let kind = normalize_kind(&term.kind);
        let target = term
            .target
            .as_deref()
            .map(|value| clamp_chars(value, MAX_TERM_CHARS))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| source.clone());
        let note = clamp_chars(term.note.as_deref().unwrap_or_default(), MAX_NOTE_CHARS);

        // Reuse an existing row so a second confirmation updates instead of
        // inserting a duplicate (the unique key includes a nullable
        // `source_lang`, which SQLite treats as distinct when NULL).
        let found = existing
            .iter()
            .find(|row| row.source.eq_ignore_ascii_case(&source) && row.kind == kind);
        repo::upsert_glossary_term(
            pool,
            &GlossaryTerm {
                id: found.map_or_else(new_id, |row| row.id.clone()),
                project_id: req.project_id.clone(),
                source_lang: found
                    .and_then(|row| row.source_lang.clone())
                    .or_else(|| glossary_source_lang.clone()),
                target_lang: Some(project.target_lang.clone()),
                source: source.clone(),
                target: target.clone(),
                note: (!note.is_empty()).then_some(note.clone()),
                kind: kind.clone(),
                origin: "proposed".to_string(),
                revision: 1,
                status: "approved".to_string(),
            },
        )
        .await?;
        written_terms.push(serde_json::json!({
            "source": source,
            "target": target,
            "kind": kind,
            "note": note,
        }));
    }

    let book_meta = serde_json::json!({
        "fields": fields,
        "proper_nouns": written_terms,
        "provenance": req.profile.provenance,
        "confirmed_at": now(),
    });
    repo::set_memory(pool, &req.project_id, META_KEY, &book_meta.to_string()).await?;

    snapshot(pool, &req.project_id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(id: &str, kind: &str, chapter: Option<&str>, order: i64, text: &str) -> Block {
        Block {
            id: id.to_string(),
            document_id: "d".to_string(),
            chapter_id: chapter.map(str::to_string),
            order_index: order,
            kind: kind.to_string(),
            level: 0,
            source_md: text.to_string(),
            source_text: text.to_string(),
            translatable: true,
            attrs_json: "{}".to_string(),
            content_hash: format!("h-{id}"),
        }
    }

    fn chapter(id: &str, order: i64, title: &str) -> Chapter {
        Chapter {
            id: id.to_string(),
            document_id: "d".to_string(),
            order_index: order,
            title: title.to_string(),
            level: 1,
            block_first: 0,
            block_last: 0,
            summary: None,
            summary_model: None,
            summary_hash: None,
            status: "pending".to_string(),
        }
    }

    #[test]
    fn excerpts_take_the_incipit_and_two_paragraphs_per_chapter() {
        let blocks = vec![
            block("b1", "heading", None, 0, "Title"),
            block("b2", "para", None, 1, "Preamble paragraph."),
            block("b3", "para", None, 2, "Another preamble."),
            block("b4", "para", None, 3, "Preamble overflow."),
            block("b5", "para", Some("c1"), 4, "Chapter one opening."),
            block("b6", "code", Some("c1"), 5, "SELECT 1;"),
            block("b7", "para", Some("c1"), 6, "Chapter one second."),
            block("b8", "para", Some("c1"), 7, "Chapter one overflow."),
            block("b9", "para", Some("c2"), 8, "Chapter two opening."),
        ];
        let chapters = vec![chapter("c1", 1, "One"), chapter("c2", 2, "Two")];
        let evidence = excerpts_from(&blocks, &chapters);

        assert!(evidence.excerpts.contains("## Incipit"));
        assert!(evidence.excerpts.contains("Preamble paragraph."));
        assert!(!evidence.excerpts.contains("Preamble overflow."));
        assert!(evidence.excerpts.contains("## One"));
        assert!(evidence.excerpts.contains("Chapter one second."));
        assert!(!evidence.excerpts.contains("Chapter one overflow."));
        // Code is never quoted as evidence.
        assert!(!evidence.excerpts.contains("SELECT 1;"));
        // Two preamble + two chapter-one + one chapter-two paragraphs.
        assert_eq!(evidence.blocks, 5);
    }

    #[test]
    fn sanitize_clamps_lengths_and_normalizes_kinds() {
        let raw = RawProfile {
            genre: "x".repeat(400),
            style_notes: (0..12).map(|i| format!("note {i}")).collect(),
            synopsis: "word ".repeat(500),
            proper_nouns: vec![
                ProperNoun {
                    source: "Elena".into(),
                    kind: "DO_NOT_TRANSLATE".into(),
                    note: "n".repeat(400),
                },
                ProperNoun {
                    source: "elena".into(),
                    kind: "proper_noun".into(),
                    note: String::new(),
                },
                ProperNoun {
                    source: "  ".into(),
                    kind: "proper_noun".into(),
                    note: String::new(),
                },
            ],
            field_basis: BTreeMap::from([("genre".to_string(), "from_text".to_string())]),
            ..RawProfile::default()
        };
        let profile = raw.sanitize(ProfileProvenance::default());

        assert_eq!(profile.genre.basis, "from_text");
        assert!(profile.genre.value.chars().count() <= MAX_SHORT_CHARS);
        assert_eq!(profile.genre.value.chars().last(), Some('…'));
        assert_eq!(profile.style_notes.value.len(), MAX_STYLE_NOTES);
        assert_eq!(profile.synopsis.basis, "inferred");
        assert!(profile.synopsis.value.split_whitespace().count() <= MAX_SYNOPSIS_WORDS);
        // Case-insensitive dedupe, leading whitespace dropped, kind normalized.
        assert_eq!(profile.proper_nouns.len(), 1);
        assert_eq!(profile.proper_nouns[0].source, "Elena");
        assert_eq!(profile.proper_nouns[0].kind, "do_not_translate");
        assert!(profile.proper_nouns[0].note.chars().count() <= MAX_NOTE_CHARS);
    }

    #[test]
    fn parse_profile_extracts_json_from_code_fences() {
        let answer =
            "```json\n{\"genre\":\"fiction\",\"field_basis\":{\"genre\":\"from_text\"}}\n```";
        let profile = parse_profile(answer, ProfileProvenance::default()).expect("parse");
        assert_eq!(profile.genre.value, "fiction");
        assert_eq!(profile.genre.basis, "from_text");
        assert!(profile.synopsis.value.is_empty());
    }

    #[test]
    fn parse_profile_rejects_prose_answers() {
        let error = parse_profile("not json at all", ProfileProvenance::default())
            .expect_err("prose must be rejected");
        assert!(matches!(error, AppError::Invalid(_)));
    }

    #[test]
    fn metadata_text_caps_and_ignores_empty_front_matter() {
        assert!(metadata_text("{}").is_empty());
        assert!(metadata_text("not json").is_empty());
        let text = metadata_text(r#"{"title":"The Book","author":"Someone"}"#);
        assert!(text.contains("\"title\": \"The Book\""));
    }

    #[test]
    fn embedded_prompts_match_the_repository_files() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let text = std::fs::read_to_string(root.join("prompts/analyze_book.md"))
            .expect("read prompts/analyze_book.md");
        let body = text
            .split_once("-->\n")
            .expect("the file must start with an HTML header")
            .1;
        let (system, user) = body
            .split_once("\n---USER---\n")
            .expect("the file must contain the ---USER--- marker");
        assert_eq!(system, DEFAULT_ANALYZE_SYSTEM_TEMPLATE);
        assert_eq!(user, DEFAULT_ANALYZE_USER_TEMPLATE);

        let file: Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("prompts/analyze_book.schema.json"))
                .expect("read prompts/analyze_book.schema.json"),
        )
        .expect("the schema file is valid JSON");
        let embedded: Value =
            serde_json::from_str(DEFAULT_ANALYZE_SCHEMA).expect("embedded schema is valid JSON");
        assert_eq!(file, embedded);
    }
}
