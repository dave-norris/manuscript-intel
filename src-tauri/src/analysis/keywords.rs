// analysis/keywords.rs — KDP keyword optimization, search term generation,
// discovery keywords, and Canopy-based keyword search.

use std::collections::HashMap;
use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager};

use super::{emit, err, extract_json_object, GenreResult};
use crate::db;
use crate::prompts;
use crate::models::KeywordResult;

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct KeywordRequest {
    pub folder:   String,
    pub api_key:  String,
    pub model:    String,
    pub provider: String,
}

// ── Tauri commands ───────────────────────────────────────────────────────────

#[tauri::command]
pub async fn generate_search_terms(app: AppHandle, request: KeywordRequest) -> GenreResult {
    let database = app.state::<db::Db>();
    let genre_data = { let conn = database.0.lock().unwrap(); db::load_genre_data(&conn, &request.folder) };
    let genre_data = match genre_data {
        Some(d) => d,
        None    => return err("No genre data found. Run Analyze first."),
    };

    emit(&app, "Generating competition search terms...");
    emit(&app, &format!("  Genre: {}", genre_data.industry_ebook));

    match generate_mi_search_terms(&database, &request.provider, &request.api_key, &request.model, &genre_data).await {
        Err(e) => err(&format!("AI error: {}", e)),
        Ok(keywords) => {
            emit(&app, &format!("  ✓ {} search terms generated:", keywords.len()));
            for kw in &keywords { emit(&app, &format!("    • {}", kw)); }

            let rendered = render_search_terms(&keywords);
            let conn = database.0.lock().unwrap();
            let _ = db::save_mi_search_terms(&conn, &request.folder, &keywords);
            let _ = db::save_document(&conn, &request.folder, "mi_search_terms", &rendered);

            GenreResult { success: true, report: rendered, error: String::new(), run_ts: String::new() }
        }
    }
}

#[tauri::command]
pub async fn optimize_keywords(app: AppHandle, request: KeywordRequest) -> GenreResult {
    let database = app.state::<db::Db>();
    let genre_data = { let conn = database.0.lock().unwrap(); db::load_genre_data(&conn, &request.folder) };
    let genre_data = match genre_data {
        Some(d) => d,
        None    => return err("No genre data found. Run Full Analysis first."),
    };

    emit(&app, "Extracting keyword material...");
    let source_note = if !genre_data.genre_signals.is_empty() {
        "*(Generated from genre analysis.)*"
    } else {
        "*(Generated from genre analysis. Run Analyze Competition for PR-sourced keywords.)*"
    };

    emit(&app, &format!("Asking {} to optimize keywords...", &request.model));

    match call_keyword_optimizer(&database, &request.provider, &request.api_key, &request.model, &genre_data, &genre_data.genre_signals).await {
        Err(e) => err(&format!("AI error: {}", e)),
        Ok((entries, strategy)) => {
            let rendered = render_kdp_keywords(&entries, &strategy, source_note);
            let conn = database.0.lock().unwrap();
            let _ = db::save_kdp_keywords(&conn, &request.folder, &entries, &strategy, source_note);
            let _ = db::save_document(&conn, &request.folder, "kdp_keywords", &rendered);
            emit(&app, "✓ KDP keywords saved to database.");
            GenreResult { success: true, report: rendered, error: String::new(), run_ts: String::new() }
        }
    }
}

// ── Core logic ───────────────────────────────────────────────────────────────

pub(crate) async fn generate_mi_search_terms(
    db: &db::Db,
    provider: &str,
    api_key: &str,
    model: &str,
    genre_data: &db::GenreDataRow,
) -> Result<Vec<String>, String> {
    let kdp_categories = genre_data.kdp_ebook.iter()
        .map(|p| p.split('>').last().unwrap_or(p).trim().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let genre_signals = &genre_data.genre_signals[..genre_data.genre_signals.len().min(500)];

    let mut vars = HashMap::new();
    vars.insert("genre", genre_data.industry_ebook.as_str());
    vars.insert("kdp_categories", kdp_categories.as_str());
    vars.insert("genre_signals", genre_signals);

    let raw = prompts::execute_prompt(db, "mi_search_terms", provider, api_key, model, vars).await?;
    let clean = raw.trim()
        .trim_start_matches("```json").trim_start_matches("```")
        .trim_end_matches("```").trim();

    serde_json::from_str::<Vec<String>>(clean)
        .map_err(|e| format!("Parse error: {} | got: {}", e, &clean[..clean.len().min(200)]))
}

pub(crate) async fn call_keyword_optimizer(
    db: &db::Db,
    provider: &str,
    api_key: &str,
    model: &str,
    genre_data: &db::GenreDataRow,
    keywords_text: &str,
) -> Result<(Vec<db::KdpKeywordEntry>, String), String>
{
    let kdp_ebook = genre_data.kdp_ebook.join(", ");
    let kdp_print = genre_data.kdp_print.join(", ");

    let mut vars = HashMap::new();
    vars.insert("industry_ebook", genre_data.industry_ebook.as_str());
    vars.insert("industry_print", genre_data.industry_print.as_str());
    vars.insert("kdp_ebook", kdp_ebook.as_str());
    vars.insert("kdp_print", kdp_print.as_str());
    vars.insert("keywords_text", keywords_text);

    let raw = prompts::execute_prompt(db, "kdp_keywords", provider, api_key, model, vars).await?;
    let clean = extract_json_object(&raw)
        .ok_or_else(|| format!("No JSON object found in response: {}", &raw[..raw.len().min(200)]))?;

    let v: serde_json::Value = serde_json::from_str(&clean)
        .map_err(|e| format!("JSON parse: {} | got: {}", e, &clean[..clean.len().min(400)]))?;

    let keywords = v["keywords"].as_array().ok_or("Missing keywords array")?;
    let strategy = v["strategy"].as_str().unwrap_or("").to_string();

    let entries: Vec<db::KdpKeywordEntry> = keywords.iter().map(|kw| {
        let s = kw["string"].as_str().unwrap_or("").to_string();
        let chars = kw["chars"].as_i64().unwrap_or(s.len() as i64);
        db::KdpKeywordEntry {
            chars: chars.max(s.len() as i64),
            string: s,
            rationale: kw["rationale"].as_str().unwrap_or("").to_string(),
        }
    }).collect();

    Ok((entries, strategy))
}

pub(crate) fn format_keyword_pool_table(pool: &[KeywordResult]) -> String {
    let mut lines = vec![
        "| Keyword | Monthly Searches | Competition | Est. Earnings |".to_string(),
        "|---------|-----------------|-------------|---------------|".to_string(),
    ];
    for r in pool.iter().take(50) {
        lines.push(format!(
            "| {} | {} | {} | {} |",
            r.keyword, r.searches, r.competition, r.estimated_earnings
        ));
    }
    lines.join("\n")
}

pub(crate) async fn call_keyword_optimizer_with_pool(
    db: &db::Db,
    provider: &str,
    api_key: &str,
    model: &str,
    genre_data: &db::GenreDataRow,
    keywords_text: &str,
    keyword_pool: &[KeywordResult],
) -> Result<(Vec<db::KdpKeywordEntry>, String), String> {
    if keyword_pool.is_empty() {
        return call_keyword_optimizer(db, provider, api_key, model, genre_data, keywords_text).await;
    }

    let pool_table = format_keyword_pool_table(keyword_pool);
    let kdp_ebook = genre_data.kdp_ebook.join(", ");
    let kdp_print = genre_data.kdp_print.join(", ");

    let mut vars = HashMap::new();
    vars.insert("industry_ebook", genre_data.industry_ebook.as_str());
    vars.insert("industry_print", genre_data.industry_print.as_str());
    vars.insert("kdp_ebook", kdp_ebook.as_str());
    vars.insert("kdp_print", kdp_print.as_str());
    vars.insert("keywords_text", keywords_text);
    vars.insert("pool_table", pool_table.as_str());

    let raw = prompts::execute_prompt(db, "kdp_keywords_with_pool", provider, api_key, model, vars).await?;
    let clean = extract_json_object(&raw)
        .ok_or_else(|| format!("No JSON object found in response: {}", &raw[..raw.len().min(200)]))?;

    let v: serde_json::Value = serde_json::from_str(&clean)
        .map_err(|e| format!("JSON parse: {} | got: {}", e, &clean[..clean.len().min(400)]))?;

    let keywords = v["keywords"].as_array().ok_or("Missing keywords array")?;
    let strategy = v["strategy"].as_str().unwrap_or("").to_string();

    let entries: Vec<db::KdpKeywordEntry> = keywords.iter().map(|kw| {
        let s = kw["string"].as_str().unwrap_or("").to_string();
        let chars = kw["chars"].as_i64().unwrap_or(s.len() as i64);
        db::KdpKeywordEntry {
            chars: chars.max(s.len() as i64),
            string: s,
            rationale: kw["rationale"].as_str().unwrap_or("").to_string(),
        }
    }).collect();

    Ok((entries, strategy))
}

/// Generate 10 discovery keyword phrases for non-Amazon platforms.
pub(crate) async fn generate_discovery_keywords(
    db: &db::Db,
    provider: &str,
    api_key: &str,
    model: &str,
    genre_data: &db::GenreDataRow,
) -> Result<Vec<db::DiscoveryKeywordEntry>, String> {
    let mut vars = HashMap::new();
    vars.insert("industry_ebook", genre_data.industry_ebook.as_str());
    vars.insert("industry_print", genre_data.industry_print.as_str());
    vars.insert("reader_demographic", genre_data.reader_demographic.as_str());
    vars.insert("bookstore_shelving", genre_data.bookstore_shelving.as_str());
    vars.insert("genre_signals", genre_data.genre_signals.as_str());

    let raw = prompts::execute_prompt(db, "discovery_keywords", provider, api_key, model, vars).await?;
    let clean = extract_json_object(&raw)
        .ok_or_else(|| format!("No JSON in discovery response: {}", &raw[..raw.len().min(200)]))?;
    let v: serde_json::Value = serde_json::from_str(&clean)
        .map_err(|e| format!("JSON parse: {} | got: {}", e, &clean[..clean.len().min(400)]))?;

    let keywords = v["keywords"].as_array()
        .ok_or_else(|| "Missing keywords array in discovery response".to_string())?;

    let entries: Vec<db::DiscoveryKeywordEntry> = keywords.iter().map(|kw| {
        db::DiscoveryKeywordEntry {
            phrase:    kw["phrase"].as_str().unwrap_or("").to_string(),
            rationale: kw["rationale"].as_str().unwrap_or("AI-reasoned").to_string(),
        }
    }).collect();

    Ok(entries)
}

/// Derive 2-3 seed terms from existing analysis data.
/// Returns a deduplicated Vec of seed strings (case-insensitive dedup).
pub(crate) fn derive_keyword_seeds(
    industry_ebook: &str,
    top_categories: &[String],
) -> Vec<String> {
    let mut seeds: Vec<String> = Vec::new();

    // Seed 1: first 3 words of industry_ebook, lowercased
    let words: Vec<&str> = industry_ebook.split_whitespace().collect();
    let genre_seed = words[..words.len().min(3)].join(" ").to_lowercase();
    if !genre_seed.is_empty() {
        seeds.push(genre_seed);
    }

    // Seeds 2-3: leaf segment (text after last ">") from top 1-2 category paths
    for cat_path in top_categories.iter().take(2) {
        let leaf = cat_path
            .rsplit('>')
            .next()
            .unwrap_or("")
            .trim()
            .to_lowercase();
        if !leaf.is_empty() {
            seeds.push(leaf);
        }
    }

    // Deduplicate (case-insensitive)
    let mut seen: Vec<String> = Vec::new();
    seeds.retain(|s| {
        let lower = s.to_lowercase();
        if seen.contains(&lower) {
            false
        } else {
            seen.push(lower);
            true
        }
    });

    seeds
}

/// Canopy-based keyword search — replaces legacy keyword search in the pipeline.
/// For each seed: gets autocomplete suggestions, searches top results, estimates volume.
pub(crate) async fn run_keyword_searches_canopy(
    app: &AppHandle,
    folder: &str,
    seeds: &[String],
    canopy_api_key: &str,
) -> Vec<KeywordResult> {
    let mut all_results: Vec<KeywordResult> = Vec::new();

    let client = match crate::canopy::CanopyClient::new(canopy_api_key) {
        Ok(c) => c,
        Err(e) => {
            let _ = app.emit("cdp:log", &format!("⚠ Could not create Canopy client: {}", e));
            return all_results;
        }
    };

    for seed in seeds {
        if crate::is_cancelled() { break; }

        let _ = app.emit("cdp:log", &format!("Keyword search (Canopy): \"{}\"", seed));

        // Get suggestions
        let suggestions = client.autocomplete(seed, "US", Some("digital-text")).await
            .unwrap_or_else(|_| vec![seed.clone()]);
        let mut keywords: Vec<String> = vec![seed.clone()];
        for s in suggestions.into_iter().take(10) {
            if !keywords.contains(&s) { keywords.push(s); }
        }

        let mut results: Vec<KeywordResult> = Vec::new();
        for kw in &keywords {
            let search = match client.search(kw, "US", Some("digital-text"), 1).await {
                Ok(r) => r,
                Err(_) => continue,
            };
            if search.is_empty() {
                results.push(KeywordResult { keyword: kw.clone(), searches: "0".to_string(), competition: "Low".to_string(), estimated_earnings: "$0".to_string() });
                continue;
            }
            let organic: Vec<_> = search.iter().filter(|r| !r.is_sponsored).take(3).collect();
            let mut daily_sales: Vec<f64> = Vec::new();
            for sr in &organic {
                if sr.asin.is_empty() { continue; }
                if let Ok(s) = client.get_sales(&sr.asin, "US").await {
                    if let Some(d) = s.estimated_daily_sales { daily_sales.push(d); }
                }
                tokio::time::sleep(std::time::Duration::from_millis(80)).await;
            }
            let avg = if daily_sales.is_empty() { 0.0 } else { daily_sales.iter().sum::<f64>() / daily_sales.len() as f64 };
            let monthly_searches = (avg * 30.0 * 33.0) as u64;
            let avg_reviews: f64 = {
                let counts: Vec<f64> = search.iter().filter_map(|r| r.review_count.map(|c| c as f64)).collect();
                if counts.is_empty() { 0.0 } else { counts.iter().sum::<f64>() / counts.len() as f64 }
            };
            let sponsored_count = search.iter().filter(|r| r.is_sponsored).count();
            let competition = if avg_reviews > 500.0 || sponsored_count > 5 { "High" }
                else if avg_reviews > 100.0 || sponsored_count > 2 { "Medium" }
                else { "Low" };
            let est_earnings = avg * 30.0 * 0.3 * 2.80;
            results.push(KeywordResult {
                keyword: kw.clone(),
                searches: format!("{}", monthly_searches),
                competition: competition.to_string(),
                estimated_earnings: format!("${:.0}", est_earnings),
            });
        }

        // Persist
        {
            let database = app.state::<crate::db::Db>();
            let conn = database.0.lock().unwrap();
            let rows: Vec<(String, String, String, String)> = results.iter()
                .map(|r| (r.keyword.clone(), r.searches.clone(), r.competition.clone(), r.estimated_earnings.clone()))
                .collect();
            let _ = crate::db::replace_keyword_search_results(&conn, folder, seed, &rows);
        }

        let _ = app.emit("cdp:log", &format!("✓ \"{}\" → {} keyword(s).", seed, results.len()));
        all_results.extend(results);
    }

    all_results
}

// ── Rendering ────────────────────────────────────────────────────────────────

pub(crate) fn render_kdp_keywords(entries: &[db::KdpKeywordEntry], strategy: &str, source_note: &str) -> String {
    let json = serde_json::json!({
        "schema": "kdp_keywords_v1",
        "source_note": source_note,
        "entries": entries.iter().enumerate().map(|(i, kw)| {
            serde_json::json!({
                "field": i + 1,
                "string": kw.string,
                "chars": kw.chars,
                "rationale": kw.rationale,
                "over_limit": kw.string.len() > 50,
            })
        }).collect::<Vec<_>>(),
        "strategy": strategy,
    });
    json.to_string()
}

pub(crate) fn render_search_terms(keywords: &[String]) -> String {
    let json = serde_json::json!({
        "schema": "mi_search_terms_v1",
        "keywords": keywords,
    });
    json.to_string()
}

/// DataForSEO-based keyword search — uses Amazon Related Keywords API.
/// Sends all seeds in one batch API call for efficiency.
pub(crate) async fn run_keyword_searches_dataforseo(
    app: &AppHandle,
    folder: &str,
    seeds: &[String],
    dataforseo_login: &str,
    dataforseo_password: &str,
) -> Vec<KeywordResult> {
    let client = match crate::dataforseo::DataForSeoClient::new(dataforseo_login, dataforseo_password) {
        Ok(c) => c,
        Err(e) => {
            let _ = app.emit("cdp:log", &format!("⚠ DataForSEO client error: {}", e));
            return Vec::new();
        }
    };

    let _ = app.emit("cdp:log", &format!("DataForSEO: Searching Amazon keywords for {} seed(s) in one batch...", seeds.len()));
    for seed in seeds {
        let _ = app.emit("cdp:log", &format!("  Seed: \"{}\"", seed));
    }

    let all_results: Vec<KeywordResult> = match client.amazon_related_keywords_batch(seeds, 20).await {
        Ok(keywords) => {
            let _ = app.emit("cdp:log", &format!("  ✓ {} keywords returned.", keywords.len()));
            keywords.into_iter().map(|kw| {
                let competition = if kw.search_volume > 50000 { "High" }
                    else if kw.search_volume > 5000 { "Medium" }
                    else { "Low" };

                KeywordResult {
                    keyword: kw.keyword,
                    searches: format!("{}", kw.search_volume),
                    competition: competition.to_string(),
                    estimated_earnings: String::new(),
                }
            }).collect()
        }
        Err(e) => {
            let _ = app.emit("cdp:log", &format!("  ⚠ DataForSEO error: {}", e));
            Vec::new()
        }
    };

    // Persist results to DB
    if !all_results.is_empty() {
        let database = app.state::<crate::db::Db>();
        let conn = database.0.lock().unwrap();
        let rows: Vec<(String, String, String, String)> = all_results.iter()
            .map(|r| (r.keyword.clone(), r.searches.clone(), r.competition.clone(), r.estimated_earnings.clone()))
            .collect();
        // Save all under the first seed as the batch key
        if let Some(first_seed) = seeds.first() {
            let _ = crate::db::replace_keyword_search_results(&conn, folder, first_seed, &rows);
        }
    }

    let _ = app.emit("cdp:log", &format!("✓ DataForSEO: {} total Amazon keywords.", all_results.len()));
    all_results
}
