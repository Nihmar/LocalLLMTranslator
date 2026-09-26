//! Prompt assembly: stable system message + budgeted volatile user message.
//!
//! Templates are Jinja2, rendered with `minijinja` so Rust and Python can share
//! the same files. They are loaded from the project's `prompts` snapshot
//! directory; if a file is missing the builder falls back to a minimal embedded
//! default so a project can always be translated.
//!
//! # Why the glossary lives in the user message
//!
//! PLAN.md section 7.2 lists the glossary in the system message, but section 9.2
//! requires the glossary to be *filtered to the terms present in each chunk*.
//! Those two rules are incompatible: a per-chunk glossary would make the system
//! message differ between chunks and destroy the stable KV-cache prefix that
//! section 7.2 exists to protect. The glossary and the synopsis therefore live in
//! the user message, and the system message only ever contains book-level
//! constants (source/target language, style guide, book title/author). This is a
//! deliberate, documented deviation; see the module tests.

use std::path::Path;

use minijinja::{context, Environment};
use serde::{Deserialize, Serialize};

use super::budget::{fill_budget, BudgetPiece, BudgetedContext, PieceKind, TokenCounter};
use crate::error::Result;

/// Minimal fallback system template (stable, no chunk-specific data).
pub const DEFAULT_SYSTEM_TEMPLATE: &str = r#"You are a professional literary translator. You translate from {{ source_language }} into {{ target_language }}.

HARD RULES
1. Output ONLY the translation of the passage in the user message. No commentary, no notes, no preface, no code fences around the result.
2. Preserve Markdown structure exactly: same number of lines and paragraphs, same list markers, same heading levels, same blank-line separation.
3. Tokens like ⟦12⟧ are placeholders. Copy each one verbatim, exactly once, in a position that is grammatical in {{ target_language }}. Never translate, split, merge, renumber or reorder them.
4. Never translate: fenced code blocks, inline code, URLs, DOIs, file paths, email addresses.
5. Use the GLOSSARY exactly as given whenever the source term occurs.
6. Do not summarise, do not omit sentences, do not merge or split paragraphs, do not add sentences that are not in the source.
7. Keep the source's paragraph rhythm and register; translate idioms into natural {{ target_language }}, not word-for-word.

STYLE GUIDE
{{ style_guide }}

BOOK
Title: {{ book_title }}
Author: {{ book_author }}"#;

/// Minimal fallback user template (volatile, per chunk).
pub const DEFAULT_USER_TEMPLATE: &str = r#"CHAPTER: {{ chapter_title }}
{% if heading_chain %}SECTION: {{ heading_chain }}
{% endif %}{% if chapter_summary_so_far %}CHAPTER SUMMARY SO FAR
{{ chapter_summary_so_far }}
{% endif %}{% if previous_chapters %}PREVIOUS CHAPTERS
{{ previous_chapters }}
{% endif %}{% if glossary %}GLOSSARY (source => target)
{{ glossary }}
{% endif %}{% if synopsis %}SYNOPSIS
{{ synopsis }}
{% endif %}{% if previous_context %}PREVIOUS PASSAGE (already translated — for continuity of tone, pronouns and terminology only; do NOT translate it):
{{ previous_context }}
{% endif %}{% if chunk_flags %}NOTE: {{ chunk_flags }}
{% endif %}
PASSAGE TO TRANSLATE:
{{ text }}
"#;

/// One glossary entry, already resolved to the project's languages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlossaryEntry {
    pub source: String,
    pub target: String,
    #[serde(default)]
    pub kind: String,
    /// Surface forms that also trigger the entry (series terms, PLAN.md §9.5).
    #[serde(default)]
    pub aliases: Vec<String>,
}

/// Everything the builder needs to render a chunk prompt.
#[derive(Debug, Clone, Default)]
pub struct ContextInputs {
    pub source_language: String,
    pub target_language: String,
    pub style_guide: String,
    pub synopsis: String,
    pub book_title: String,
    pub book_author: String,
    pub glossary: Vec<GlossaryEntry>,
    pub previous_chapter_summaries: Vec<String>,
    pub rolling_summary: String,
    pub previous_tail: String,
    pub chapter_title: String,
    /// Heading chain the chunk sits in (`context_carrier`), `A › B › C`.
    pub heading_chain: String,
    /// Human-readable note about split/oversized chunks, empty when none apply.
    pub chunk_flags: String,
    pub chunk_text: String,
    pub budget_tokens: usize,
}

/// Rendered prompt plus the manifest of what was injected.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltPrompt {
    pub system: String,
    pub user: String,
    pub manifest: BudgetedContext,
}

impl BuiltPrompt {
    /// The full prompt hash input: system and user concatenated.
    pub fn full_text(&self) -> String {
        format!("{}\n\u{0}\n{}", self.system, self.user)
    }
}

/// Renders translator prompts. Cheap to clone.
#[derive(Debug, Clone)]
pub struct ContextBuilder {
    system_template: String,
    user_template: String,
}

impl Default for ContextBuilder {
    fn default() -> Self {
        Self::embedded()
    }
}

impl ContextBuilder {
    /// Use the built-in fallback templates.
    pub fn embedded() -> Self {
        Self {
            system_template: DEFAULT_SYSTEM_TEMPLATE.to_string(),
            user_template: DEFAULT_USER_TEMPLATE.to_string(),
        }
    }

    /// Load templates from a prompts snapshot directory, falling back to the
    /// embedded defaults for any missing file.
    pub fn load(dir: &Path) -> Self {
        let system_template = read_first(
            dir,
            &[
                "translator.system.md",
                "translator_system.md",
                "translator.md",
            ],
        )
        .unwrap_or_else(|| DEFAULT_SYSTEM_TEMPLATE.to_string());
        let user_template = read_first(dir, &["translator.user.md", "translator_user.md"])
            .unwrap_or_else(|| DEFAULT_USER_TEMPLATE.to_string());
        Self {
            system_template,
            user_template,
        }
    }

    /// Templates currently in use, for diagnostics.
    pub fn templates(&self) -> (&str, &str) {
        (&self.system_template, &self.user_template)
    }

    /// Render the system message. Depends only on book-level constants, so it is
    /// byte-identical for every chunk of the same book.
    pub fn render_system(&self, inputs: &ContextInputs) -> Result<String> {
        let env = Environment::new();
        let rendered = env.render_str(
            &self.system_template,
            context! {
                source_language => &inputs.source_language,
                target_language => &inputs.target_language,
                style_guide => &inputs.style_guide,
                book_title => &inputs.book_title,
                book_author => &inputs.book_author,
            },
        )?;
        Ok(rendered)
    }

    /// The system message plus the pieces that will be budgeted, without
    /// filling anything. Exposed so a caller can count the exact tokens of
    /// every piece with `/tokenize` before deciding what fits.
    pub fn pieces(&self, inputs: &ContextInputs) -> Result<(String, Vec<BudgetPiece>)> {
        let system = self.render_system(inputs)?;
        let glossary = filter_glossary(&inputs.glossary, &inputs.chunk_text);
        let glossary_text = render_glossary(&glossary);
        let previous_chapters = inputs.previous_chapter_summaries.join("\n");

        let pieces = vec![
            BudgetPiece::new(PieceKind::Text, inputs.chunk_text.clone()),
            BudgetPiece::new(PieceKind::SystemRules, system.clone()),
            BudgetPiece::new(PieceKind::ChunkFlags, inputs.chunk_flags.clone()),
            BudgetPiece::new(PieceKind::HeadingChain, inputs.heading_chain.clone()),
            BudgetPiece::new(PieceKind::Glossary, glossary_text),
            BudgetPiece::new(PieceKind::Synopsis, inputs.synopsis.clone()),
            BudgetPiece::new(PieceKind::PreviousChapters, previous_chapters),
            BudgetPiece::new(PieceKind::RollingSummary, inputs.rolling_summary.clone()),
            BudgetPiece::new(PieceKind::PreviousTail, inputs.previous_tail.clone()),
        ];
        Ok((system, pieces))
    }

    /// Fill an already-built piece list and render the user message. See
    /// [`Self::pieces`].
    pub fn build_from_pieces(
        &self,
        inputs: &ContextInputs,
        system: String,
        pieces: Vec<BudgetPiece>,
        counter: &dyn TokenCounter,
    ) -> Result<BuiltPrompt> {
        let manifest = fill_budget(pieces, inputs.budget_tokens, counter);

        let env = Environment::new();
        let user = env.render_str(
            &self.user_template,
            context! {
                chapter_title => &inputs.chapter_title,
                heading_chain => manifest.text(PieceKind::HeadingChain),
                chunk_flags => manifest.text(PieceKind::ChunkFlags),
                chapter_summary_so_far => manifest.text(PieceKind::RollingSummary),
                previous_chapters => manifest.text(PieceKind::PreviousChapters),
                glossary => manifest.text(PieceKind::Glossary),
                synopsis => manifest.text(PieceKind::Synopsis),
                previous_context => manifest.text(PieceKind::PreviousTail),
                text => manifest.text(PieceKind::Text),
            },
        )?;

        Ok(BuiltPrompt {
            system,
            user,
            manifest,
        })
    }

    /// Build the full prompt for one chunk.
    pub fn build(&self, inputs: &ContextInputs, counter: &dyn TokenCounter) -> Result<BuiltPrompt> {
        let (system, pieces) = self.pieces(inputs)?;
        self.build_from_pieces(inputs, system, pieces, counter)
    }
}

/// Keep only the glossary terms that actually occur in the chunk text, ordered
/// deterministically by source term (case-insensitive).
///
/// Matching is case-insensitive but respects word boundaries: `king` must match "the
/// king" and not "asking" or "kings", or the prompt would carry terms the chunk does
/// not contain (and the sidecar's QA check uses the same word-boundary rule).
pub fn filter_glossary(glossary: &[GlossaryEntry], chunk_text: &str) -> Vec<GlossaryEntry> {
    let haystack = chunk_text.to_lowercase();
    let mut kept: Vec<GlossaryEntry> = glossary
        .iter()
        .filter(|entry| {
            let term = entry.source.trim().to_lowercase();
            let main = !term.is_empty() && contains_term(&haystack, &term);
            main || entry.aliases.iter().any(|alias| {
                let alias = alias.trim().to_lowercase();
                !alias.is_empty() && contains_term(&haystack, &alias)
            })
        })
        .cloned()
        .collect();
    kept.sort_by(|a, b| {
        a.source
            .to_lowercase()
            .cmp(&b.source.to_lowercase())
            .then_with(|| a.source.cmp(&b.source))
    });
    kept
}

/// Whether `term_lower` (already lowercased) occurs in `haystack` as a whole word.
///
/// A manual scan instead of a regex: the glossary can hold hundreds of terms and the
/// haystack is one chunk, so rebuilding a pattern per term would cost more than the
/// boundary checks.
fn contains_term(haystack: &str, term_lower: &str) -> bool {
    let is_boundary = |c: char| !(c.is_alphanumeric() || c == '_');
    let mut search_from = 0;
    while let Some(offset) = haystack[search_from..].find(term_lower) {
        let begin = search_from + offset;
        let end = begin + term_lower.len();
        let before_ok = haystack[..begin]
            .chars()
            .next_back()
            .is_none_or(is_boundary);
        let after_ok = haystack[end..].chars().next().is_none_or(is_boundary);
        if before_ok && after_ok {
            return true;
        }
        // Advance by one whole character: `begin + 1` could slice a multi-byte
        // character in half when the rejected match starts with one.
        search_from = begin + haystack[begin..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// Render glossary entries as `source => target` lines.
pub fn render_glossary(entries: &[GlossaryEntry]) -> String {
    entries
        .iter()
        .map(|e| format!("{} => {}", e.source, e.target))
        .collect::<Vec<_>>()
        .join("\n")
}

fn read_first(dir: &Path, names: &[&str]) -> Option<String> {
    for name in names {
        let path = dir.join(name);
        if let Ok(content) = std::fs::read_to_string(&path) {
            return Some(content);
        }
    }
    None
}

/// Write the shipped translator prompt halves into a project snapshot, but never
/// overwrite a file that already exists.
///
/// The prompts are user data: a re-ingest must not silently discard an edit the
/// user made in the project snapshot. The other prompt families (editor,
/// proofreader, summarizer, reconnaissance) already follow this rule.
pub async fn ensure_prompt_files(dir: &Path) -> Result<()> {
    tokio::fs::create_dir_all(dir).await?;
    for (name, content) in [
        ("translator.system.md", DEFAULT_SYSTEM_TEMPLATE),
        ("translator.user.md", DEFAULT_USER_TEMPLATE),
    ] {
        let path = dir.join(name);
        if !path.exists() {
            tokio::fs::write(&path, content).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::budget::{CharsAsTokens, HeuristicCounter};

    fn base_inputs() -> ContextInputs {
        ContextInputs {
            source_language: "English".into(),
            target_language: "Italian".into(),
            style_guide: "Neutral, literary.".into(),
            synopsis: "A king and a queen.".into(),
            book_title: "The Book".into(),
            book_author: "Someone".into(),
            glossary: vec![
                GlossaryEntry {
                    source: "king".into(),
                    target: "re".into(),
                    kind: "term".into(),
                    aliases: Vec::new(),
                },
                GlossaryEntry {
                    source: "dragon".into(),
                    target: "drago".into(),
                    kind: "term".into(),
                    aliases: Vec::new(),
                },
            ],
            previous_chapter_summaries: vec!["Chapter one summary.".into()],
            rolling_summary: "So far, the king spoke.".into(),
            previous_tail: "…and then he left.".into(),
            chapter_title: "Chapter One".into(),
            heading_chain: "Chapter One › The Harbour".into(),
            chunk_flags: String::new(),
            chunk_text: "The king spoke to the court.".into(),
            budget_tokens: 1000,
        }
    }

    #[test]
    fn glossary_is_filtered_to_terms_present_in_chunk() {
        let glossary = vec![
            GlossaryEntry {
                source: "king".into(),
                target: "re".into(),
                kind: "term".into(),
                aliases: Vec::new(),
            },
            GlossaryEntry {
                source: "dragon".into(),
                target: "drago".into(),
                kind: "term".into(),
                aliases: Vec::new(),
            },
        ];
        let filtered = filter_glossary(&glossary, "The king spoke to the court.");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].source, "king");
        assert_eq!(render_glossary(&filtered), "king => re");
    }

    #[test]
    fn glossary_filter_respects_word_boundaries() {
        let glossary = vec![
            GlossaryEntry {
                source: "king".into(),
                target: "re".into(),
                kind: "term".into(),
                aliases: Vec::new(),
            },
            GlossaryEntry {
                source: "sea".into(),
                target: "mare".into(),
                kind: "term".into(),
                aliases: Vec::new(),
            },
        ];
        // Substrings inside longer words are not occurrences.
        assert!(filter_glossary(&glossary, "He was asking about the seas.").is_empty());
        // Real occurrences still match, case-insensitively and across punctuation.
        let filtered = filter_glossary(&glossary, "The KING looked at the sea, then at (Kingdom).");
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].source, "king");
        assert_eq!(filtered[1].source, "sea");

        let multi = vec![GlossaryEntry {
            source: "old town".into(),
            target: "città vecchia".into(),
            kind: "term".into(),
            aliases: Vec::new(),
        }];
        assert_eq!(filter_glossary(&multi, "the (old town), at dawn").len(), 1);
        assert!(filter_glossary(&multi, "the oldtown clock").is_empty());
    }

    #[test]
    fn glossary_aliases_trigger_the_entry() {
        let glossary = vec![GlossaryEntry {
            source: "Keeper".into(),
            target: "Custode".into(),
            kind: "term".into(),
            aliases: vec!["the Keeper".into(), "Keeper's".into()],
        }];
        // The canonical form and every alias match...
        assert_eq!(
            filter_glossary(&glossary, "The Keeper watched the light.").len(),
            1
        );
        assert_eq!(filter_glossary(&glossary, "the Keeper's log").len(), 1);
        assert_eq!(
            filter_glossary(&glossary, "A keeper of secrets").len(),
            1,
            "the canonical source still matches directly"
        );
        // ...but a longer word containing one of them does not.
        assert!(filter_glossary(&glossary, "The keepers were many.").is_empty());
        // The rendered pair is the canonical one, never the alias.
        assert_eq!(
            render_glossary(&filter_glossary(&glossary, "the Keeper's log")),
            "Keeper => Custode"
        );
    }

    #[test]
    fn system_message_is_byte_identical_across_chunks_of_the_same_book() {
        let builder = ContextBuilder::embedded();
        let counter = HeuristicCounter;

        // Two chunks of the same book with different volatile inputs, and even a
        // different glossary / synopsis, to make sure nothing chunk- or
        // glossary-dependent leaks into the system message.
        let mut first = base_inputs();
        let mut second = base_inputs();
        second.chunk_text = "A dragon flew at dawn.".into();
        second.chapter_title = "Chapter Two".into();
        second.previous_tail = "completely different tail".into();
        second.rolling_summary = "different rolling summary".into();
        second.glossary = vec![GlossaryEntry {
            source: "dragon".into(),
            target: "drago".into(),
            kind: "term".into(),
            aliases: Vec::new(),
        }];
        second.synopsis = "A totally different synopsis.".into();

        let p1 = builder.build(&first, &counter).expect("build 1");
        let p2 = builder.build(&second, &counter).expect("build 2");

        assert_eq!(p1.system, p2.system, "system message must be stable");
        assert_ne!(p1.user, p2.user, "user message must differ per chunk");

        // Sanity: a naive implementation could accidentally match; make the
        // requirement explicit by checking no volatile data is present.
        assert!(!p1.system.contains("king"));
        assert!(!p1.system.contains("dragon"));
        assert!(!p1.system.contains("synopsis"));

        // And it is genuinely stable when nothing book-level changes.
        first.chunk_text = "something else entirely".into();
        let p3 = builder.build(&first, &counter).expect("build 3");
        assert_eq!(p1.system, p3.system);
    }

    #[test]
    fn budget_is_respected_and_low_priorities_drop_first() {
        let builder = ContextBuilder::embedded();
        let counter = CharsAsTokens;
        let mut inputs = base_inputs();
        inputs.previous_tail = "x".repeat(400);
        inputs.rolling_summary = "y".repeat(400);

        // The system prefix and the passage are required and are never dropped
        // (a request without them is meaningless), so the budget has to cover them
        // before it can say anything about the optional pieces. Give it a little
        // room on top, which is what the glossary is allowed to consume.
        let required = counter.count(&builder.render_system(&inputs).expect("render"))
            + counter.count(&inputs.chunk_text);
        inputs.budget_tokens = required + 20;

        let built = builder.build(&inputs, &counter).expect("build");
        assert!(
            built.manifest.total_tokens <= inputs.budget_tokens,
            "total {} exceeded budget {}",
            built.manifest.total_tokens,
            inputs.budget_tokens
        );
        assert!(built.manifest.included(PieceKind::Text));
        assert!(built.manifest.included(PieceKind::SystemRules));
        // The volatile tail is the first thing to go.
        assert!(!built.manifest.included(PieceKind::RollingSummary));
        assert!(!built.manifest.included(PieceKind::PreviousTail));
    }

    #[test]
    fn required_pieces_survive_a_budget_too_small_for_them() {
        let builder = ContextBuilder::embedded();
        let counter = CharsAsTokens;
        let mut inputs = base_inputs();
        inputs.budget_tokens = 1;
        inputs.previous_tail = "x".repeat(400);

        let built = builder.build(&inputs, &counter).expect("build");
        // Documented contract: overshooting the budget is preferable to emitting a
        // request with no system rules or no passage to translate.
        assert!(built.manifest.total_tokens > inputs.budget_tokens);
        assert!(built.manifest.included(PieceKind::Text));
        assert!(built.manifest.included(PieceKind::SystemRules));
        assert!(!built.manifest.included(PieceKind::PreviousTail));
    }

    #[test]
    fn pieces_and_build_from_pieces_match_build() {
        let builder = ContextBuilder::embedded();
        let counter = HeuristicCounter;
        let inputs = base_inputs();

        let direct = builder.build(&inputs, &counter).expect("build");
        let (system, pieces) = builder.pieces(&inputs).expect("pieces");
        let rebuilt = builder
            .build_from_pieces(&inputs, system, pieces, &counter)
            .expect("build from pieces");

        assert_eq!(direct.system, rebuilt.system);
        assert_eq!(direct.user, rebuilt.user);
        assert_eq!(direct.manifest.total_tokens, rebuilt.manifest.total_tokens);
    }

    #[test]
    fn chunk_flags_and_heading_chain_reach_the_user_message() {
        let builder = ContextBuilder::embedded();
        let counter = HeuristicCounter;
        let mut inputs = base_inputs();
        inputs.chunk_flags = "This passage is part 2/3 of a table: repeat the header row.".into();

        let built = builder.build(&inputs, &counter).expect("build");
        assert!(built.user.contains("SECTION: Chapter One › The Harbour"));
        assert!(built.user.contains("part 2/3 of a table"));
        // The chunk-local pieces sit just above the long-range context.
        assert!(built.manifest.included(PieceKind::ChunkFlags));
        assert!(built.manifest.included(PieceKind::HeadingChain));
        let priorities: Vec<u8> = built
            .manifest
            .pieces
            .iter()
            .map(|piece| piece.priority)
            .collect();
        assert_eq!(priorities, vec![0, 1, 2, 3, 4, 5, 6, 7, 8]);

        // Empty pieces render no line at all.
        inputs.chunk_flags = String::new();
        let built = builder.build(&inputs, &counter).expect("build");
        assert!(!built.user.contains("NOTE:"));
    }

    #[test]
    fn embedded_translator_prompts_match_the_repository_file() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let file = std::fs::read_to_string(root.join("prompts/translator.md"))
            .expect("read prompts/translator.md");
        let (system, user) = file
            .split_once("-->\n")
            .expect("translator header")
            .1
            .split_once("\n---USER---\n")
            .expect("the file must contain the ---USER--- marker");
        assert_eq!(system, DEFAULT_SYSTEM_TEMPLATE);
        assert_eq!(user, DEFAULT_USER_TEMPLATE);
    }

    #[test]
    fn loads_templates_from_snapshot_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("translator.system.md"),
            "SYS {{ target_language }}|{{ style_guide }}",
        )
        .expect("write");
        std::fs::write(dir.path().join("translator.user.md"), "USR {{ text }}").expect("write");
        let builder = ContextBuilder::load(dir.path());
        let counter = HeuristicCounter;
        let built = builder.build(&base_inputs(), &counter).expect("build");
        assert!(built.system.starts_with("SYS Italian|"));
        assert!(built.user.starts_with("USR The king spoke"));
    }

    #[tokio::test]
    async fn ensure_prompt_files_preserves_user_edits() {
        let dir = tempfile::tempdir().expect("tempdir");
        ensure_prompt_files(dir.path()).await.expect("first");
        let system = dir.path().join("translator.system.md");
        std::fs::write(&system, "EDITED BY THE USER").expect("edit");

        // A re-ingest (or any other ensure call) must not touch the edit.
        ensure_prompt_files(dir.path()).await.expect("second");
        assert_eq!(
            std::fs::read_to_string(&system).expect("read"),
            "EDITED BY THE USER"
        );
        assert!(dir.path().join("translator.user.md").is_file());
    }
}
