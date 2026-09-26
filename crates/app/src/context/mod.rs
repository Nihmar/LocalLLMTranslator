//! Context assembly: prompt templates and priority-based budgeting.

pub mod budget;
pub mod builder;

pub use budget::{
    fill_budget, BudgetPiece, BudgetedContext, CharsAsTokens, HeuristicCounter, PieceKind,
    PieceReport, TokenCounter,
};
pub use builder::{
    filter_glossary, render_glossary, BuiltPrompt, ContextBuilder, ContextInputs, GlossaryEntry,
};
