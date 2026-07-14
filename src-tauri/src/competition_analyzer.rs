// competition_analyzer.rs — Shared types for competition analysis
//
// The actual analysis is performed via Canopy API (see canopy.rs).
// This module holds the shared data structures.

use serde::{Deserialize, Serialize};

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CompetitorBook {
    pub title:        String,
    pub subtitle:     String,
    pub review_score: String,
    pub ratings:      String,
    pub author:       String,
    pub age:          String,
    pub absr:         String,
    pub pages:        String,
    pub kwt:          String,
    pub price:        String,
    pub dy_sales:     String,
    pub mo_sales:     String,
    pub amazon_url:   String,
    pub keyword:      String,
    pub cover_url:    String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CategoryRow {
    pub category:      String,
    pub sales_to_one:  String,
    pub sales_to_ten:  String,
    pub publisher_pct: String,
    pub ku_pct:        String,
    pub keyword:       String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CompetitionData {
    pub generated:         String,
    pub keywords_analyzed: Vec<String>,
    pub books:             Vec<CompetitorBook>,
    pub categories:        Vec<CategoryRow>,
}

#[derive(Serialize)]
pub struct CompetitionResult {
    pub success: bool,
    pub report:  String,
    pub error:   String,
}
