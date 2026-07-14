// models.rs — Shared types used across modules

use serde::Serialize;

/// A single keyword result with search volume and competition data.
#[derive(Serialize, Clone, Debug)]
pub struct KeywordResult {
    pub keyword:            String,
    pub searches:           String,
    pub competition:        String,
    pub estimated_earnings: String,
}

/// Response from keyword search commands.
#[derive(Serialize)]
pub struct KeywordSearchResponse {
    pub success: bool,
    pub results: Vec<KeywordResult>,
    pub error:   String,
}
