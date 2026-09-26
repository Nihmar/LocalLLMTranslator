//! Priority-based context budgeting (PLAN.md section 9.2).
//!
//! Pieces are filled highest-priority first and truncated/omitted starting from
//! the lowest priority, so the system rules and the style guide always survive
//! while the volatile tail is the first thing to go.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::util::sha256_hex_str;

/// Fallback context budget when `/props` is not reachable (PLAN.md section 9.1).
pub const DEFAULT_CHUNK_BUDGET: usize = 6000;

/// Fraction of `n_ctx` the chunk budget may use: the rest is reserved for the
/// model's answer and for the prompt's own framing (PLAN.md section 9.1).
pub const BUDGET_FRACTION: f64 = 0.6;

/// Budget derived from the server's context size, never below a sane floor.
pub fn budget_from_n_ctx(n_ctx: u32) -> usize {
    ((f64::from(n_ctx) * BUDGET_FRACTION) as usize).max(256)
}

/// Token counting is injectable so the budget logic is testable without a
/// running `llama-server` (which is what `/tokenize` would otherwise require).
pub trait TokenCounter: Send + Sync {
    fn count(&self, text: &str) -> usize;
}

/// Heuristic counter used when `/tokenize` is unavailable: roughly 3.5
/// characters per token (PLAN.md section 7.1).
#[derive(Debug, Default, Clone, Copy)]
pub struct HeuristicCounter;

impl TokenCounter for HeuristicCounter {
    fn count(&self, text: &str) -> usize {
        (text.chars().count() as f64 / 3.5).ceil() as usize
    }
}

/// Counts a fixed number of tokens per character run; used by tests.
#[derive(Debug, Default, Clone, Copy)]
pub struct CharsAsTokens;

impl TokenCounter for CharsAsTokens {
    fn count(&self, text: &str) -> usize {
        text.chars().count()
    }
}

/// Exact for the texts `/tokenize` answered for, heuristic for everything else
/// (truncated pieces and endpoints without the route).
#[derive(Debug, Clone, Default)]
pub struct CachedCounter {
    counts: HashMap<String, usize>,
}

impl CachedCounter {
    pub fn new(counts: HashMap<String, usize>) -> Self {
        Self { counts }
    }
}

impl TokenCounter for CachedCounter {
    fn count(&self, text: &str) -> usize {
        self.counts
            .get(text)
            .copied()
            .unwrap_or_else(|| HeuristicCounter.count(text))
    }
}

/// The ordered set of context components. The numeric priority is the fill
/// order from PLAN.md section 9.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PieceKind {
    /// The passage to translate. Anchor, always included.
    Text,
    /// System prompt + hard rules + style guide (priority 1). Byte-identical for
    /// every chunk of a book so `llama-server` reuses its KV-cache prefix.
    SystemRules,
    /// How to treat a split/oversized chunk (`table_part:2/3`, `continues`, ...).
    /// Tiny, so it is paid before any long-range context.
    ChunkFlags,
    /// The chain of headings the chunk sits in (PLAN.md §9.1 item 6).
    HeadingChain,
    /// Glossary terms that occur in the chunk.
    Glossary,
    /// Book synopsis.
    Synopsis,
    /// Summaries of the previous chapters.
    PreviousChapters,
    /// Rolling summary of the current chapter.
    RollingSummary,
    /// Tail of the previous translated passage.
    PreviousTail,
}

impl PieceKind {
    /// Lower number == filled earlier and truncated later. The long-range context
    /// keeps the PLAN.md §9.2 relative order; the chunk-local pieces (flags, heading
    /// chain) are inserted right after the required prefix because they are cheap and
    /// orient the model on the passage at hand.
    pub fn priority(self) -> u8 {
        match self {
            PieceKind::Text => 0,
            PieceKind::SystemRules => 1,
            PieceKind::ChunkFlags => 2,
            PieceKind::HeadingChain => 3,
            PieceKind::Glossary => 4,
            PieceKind::Synopsis => 5,
            PieceKind::PreviousChapters => 6,
            PieceKind::RollingSummary => 7,
            PieceKind::PreviousTail => 8,
        }
    }

    /// Whether the piece must survive budgeting. The system prefix and the
    /// passage itself define the request and are never dropped.
    pub fn is_required(self) -> bool {
        matches!(self, PieceKind::Text | PieceKind::SystemRules)
    }
}

/// A candidate context component.
#[derive(Debug, Clone)]
pub struct BudgetPiece {
    pub kind: PieceKind,
    pub text: String,
}

impl BudgetPiece {
    pub fn new(kind: PieceKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
        }
    }
}

/// Per-piece record persisted as the chunk's context manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PieceReport {
    pub name: PieceKind,
    pub priority: u8,
    /// Tokens actually used (0 when omitted).
    pub tokens: usize,
    pub included: bool,
    /// True when the piece had to be shortened to fit.
    pub truncated: bool,
    /// SHA-256 of the piece as it was included.
    pub hash: String,
    /// The included text. Kept out of the persisted manifest.
    #[serde(skip)]
    pub text: String,
}

/// Result of filling a budget.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetedContext {
    pub pieces: Vec<PieceReport>,
    pub total_tokens: usize,
    pub budget_tokens: usize,
}

impl BudgetedContext {
    /// Text actually included for a given piece (possibly truncated, or empty).
    pub fn text(&self, kind: PieceKind) -> &str {
        self.pieces
            .iter()
            .find(|p| p.name == kind)
            .map(|p| p.text.as_str())
            .unwrap_or("")
    }

    pub fn included(&self, kind: PieceKind) -> bool {
        self.pieces
            .iter()
            .find(|p| p.name == kind)
            .is_some_and(|p| p.included)
    }
}

/// Truncate `text` line by line from the end until it fits in `max_tokens`.
fn truncate_to_tokens(text: &str, max_tokens: usize, counter: &dyn TokenCounter) -> String {
    if counter.count(text) <= max_tokens {
        return text.to_string();
    }
    let mut out = String::new();
    for line in text.lines() {
        let candidate = if out.is_empty() {
            line.to_string()
        } else {
            format!("{out}\n{line}")
        };
        if counter.count(&candidate) > max_tokens {
            break;
        }
        out = candidate;
    }
    out
}

/// Fill `budget_tokens` with `pieces`, truncating from the lowest priority.
pub fn fill_budget(
    pieces: Vec<BudgetPiece>,
    budget_tokens: usize,
    counter: &dyn TokenCounter,
) -> BudgetedContext {
    let mut ordered = pieces;
    ordered.sort_by_key(|p| p.kind.priority());

    let mut remaining = budget_tokens;
    let mut total_tokens = 0usize;
    let mut overflowed = false;
    let mut reports = Vec::with_capacity(ordered.len());

    for piece in ordered {
        let priority = piece.kind.priority();
        let required = piece.kind.is_required();
        let tokens = counter.count(&piece.text);

        if required || (!overflowed && tokens <= remaining) {
            total_tokens += tokens;
            remaining = remaining.saturating_sub(tokens);
            reports.push(PieceReport {
                name: piece.kind,
                priority,
                tokens,
                included: true,
                truncated: false,
                hash: sha256_hex_str(&piece.text),
                text: piece.text,
            });
            continue;
        }

        if overflowed || remaining == 0 {
            reports.push(omitted(piece.kind, priority));
            continue;
        }

        // The piece does not fully fit: truncate it to what is left, then stop
        // including any lower priority piece.
        let truncated = truncate_to_tokens(&piece.text, remaining, counter);
        if truncated.is_empty() {
            reports.push(omitted(piece.kind, priority));
        } else {
            let used = counter.count(&truncated);
            total_tokens += used;
            remaining = remaining.saturating_sub(used);
            reports.push(PieceReport {
                name: piece.kind,
                priority,
                tokens: used,
                included: true,
                truncated: true,
                hash: sha256_hex_str(&truncated),
                text: truncated,
            });
        }
        overflowed = true;
    }

    BudgetedContext {
        pieces: reports,
        total_tokens,
        budget_tokens,
    }
}

fn omitted(kind: PieceKind, priority: u8) -> PieceReport {
    PieceReport {
        name: kind,
        priority,
        tokens: 0,
        included: false,
        truncated: false,
        hash: sha256_hex_str(""),
        text: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(kind: PieceKind, n_chars: usize) -> BudgetPiece {
        let fill = "x".repeat(n_chars);
        BudgetPiece::new(kind, format!("{kind:?}{fill}"))
    }

    #[test]
    fn lowest_priority_is_truncated_first() {
        let counter = CharsAsTokens;
        let pieces = vec![
            piece(PieceKind::Text, 5),
            piece(PieceKind::SystemRules, 5),
            piece(PieceKind::Glossary, 20),
            piece(PieceKind::Synopsis, 20),
            piece(PieceKind::PreviousChapters, 20),
            piece(PieceKind::RollingSummary, 20),
            piece(PieceKind::PreviousTail, 20),
        ];
        // The two required pieces consume a little over 20 tokens (each label
        // adds a few characters); leave room for the glossary, nothing more.
        let budget = 30 + 30;
        let built = fill_budget(pieces, budget, &counter);

        assert!(built.included(PieceKind::Text));
        assert!(built.included(PieceKind::SystemRules));
        assert!(built.included(PieceKind::Glossary));
        // Everything below the glossary's priority is dropped or truncated.
        assert!(!built.included(PieceKind::PreviousTail));
        assert!(built.total_tokens <= budget);
    }

    #[test]
    fn required_pieces_survive_a_tiny_budget() {
        let counter = CharsAsTokens;
        let pieces = vec![
            piece(PieceKind::Text, 5),
            piece(PieceKind::SystemRules, 5),
            piece(PieceKind::PreviousTail, 100),
        ];
        let built = fill_budget(pieces, 1, &counter);
        assert!(built.included(PieceKind::SystemRules));
        assert!(built.included(PieceKind::Text));
        assert!(!built.included(PieceKind::PreviousTail));
    }

    #[test]
    fn budget_from_n_ctx_takes_the_documented_fraction() {
        assert_eq!(budget_from_n_ctx(32_768), 19_660);
        // The floor keeps a tiny context usable.
        assert_eq!(budget_from_n_ctx(100), 256);
    }

    #[test]
    fn cached_counter_prefers_exact_counts_and_falls_back() {
        let mut counts = HashMap::new();
        counts.insert("long text".to_string(), 2);
        let counter = CachedCounter::new(counts);
        assert_eq!(counter.count("long text"), 2);
        // Unknown texts (for example a truncated piece) use the heuristic.
        assert_eq!(
            counter.count("a text /tokenize never saw"),
            HeuristicCounter.count("a text /tokenize never saw")
        );
    }

    #[test]
    fn manifest_hashes_are_stable() {
        let counter = CharsAsTokens;
        let build = |tail: &str| {
            fill_budget(
                vec![
                    piece(PieceKind::Text, 4),
                    BudgetPiece::new(PieceKind::PreviousTail, tail),
                ],
                1000,
                &counter,
            )
        };
        let a = build("hello");
        let b = build("hello");
        let c = build("world");
        assert_eq!(
            a.text(PieceKind::PreviousTail),
            b.text(PieceKind::PreviousTail)
        );
        assert_ne!(
            a.pieces
                .iter()
                .find(|p| p.name == PieceKind::PreviousTail)
                .unwrap()
                .hash,
            c.pieces
                .iter()
                .find(|p| p.name == PieceKind::PreviousTail)
                .unwrap()
                .hash
        );
    }
}
