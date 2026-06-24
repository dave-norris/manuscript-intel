// models.rs — Model selection constants
//
// The Settings panel lets the user choose the "analysis model" — used for
// tasks requiring deep reasoning. Simple, repetitive, or structured tasks
// always use Haiku regardless of the user's setting.

/// Fast, cheap model. Used for:
/// - Chapter summarization (Phase 1, ~30 calls)
/// - Category Finder top-level ranking (simple classification)
/// - Category Finder subcategory matching (pick from a numbered list)
pub const HAIKU: &str = "claude-haiku-4-5-20251001";

/// The user's chosen analysis model (Sonnet or Opus). Used for:
/// - Genre analysis Phase 2 (nuanced literary reasoning)
/// - CSV Analyzer (competitive market analysis)
/// - Category Finder genre description (if user wants higher quality)
///
/// Passed in from the frontend settings; falls back to Sonnet if empty.
pub const SONNET: &str = "claude-sonnet-4-6";

/// Ensure a model string is valid; fall back to Sonnet if not.
pub fn resolve_analysis_model(model: &str) -> &str {
    match model {
        "claude-haiku-4-5-20251001" |
        "claude-sonnet-4-6"         |
        "claude-opus-4-6"           |
        "claude-opus-4-7"           |
        "claude-opus-4-8"           => model,
        _ => SONNET,
    }
}
