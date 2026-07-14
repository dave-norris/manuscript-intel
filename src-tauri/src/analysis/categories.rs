// analysis/categories.rs — Category finding, matching, and verification
//
// Matches a story's ranked genres against the imported KDP category catalog,
// gates on honest fit, then verifies live discoverability via Canopy API.

use serde::Deserialize;
use tauri::{AppHandle, Manager};

use super::{emit, err, GenreResult};
use crate::commands::call_llm;
use crate::db;
use crate::canopy;

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct FindCategoriesRequest {
    pub folder:   String,
    pub store:    String,  // "Kindle" or "Books"
    pub api_key:  String,
    pub model:    String,
    pub provider: String,
    #[serde(default)]
    pub canopy_api_key: String,
}

/// A candidate that cleared the fit gate, with its live
/// discoverability data (if it could be confirmed) and computed score.
pub(crate) struct QualifiedCategory {
    pub path:                   String,
    pub fit_confidence:         u8,
    pub agreeing_genres:        Vec<String>,
    pub verified:               bool,
    pub sales_to_one:           String,
    pub sales_to_ten:           String,
    pub publisher_pct:          String,
    pub ku_pct:                 String,
    pub discoverability_score:  i32,
    pub top_books:              Vec<canopy::TopBook>,
}

pub(crate) struct StoreMatchResult {
    pub per_genre:  Vec<(String, u8, Vec<(String, u8, String)>)>,  // genre, genre_conf, picks(path,conf,reason)
    pub qualifying: Vec<(String, u8, Vec<String>)>,                 // path, fit_confidence, agreeing genres
}

#[derive(Deserialize)]
struct CatalogMatch { index: usize, confidence: u8, reason: String }

#[derive(Deserialize)]
pub struct VerifyMappedRequest {
    pub folder: String,
    pub store:  String,
}

const FIT_CONFIDENCE_BAR: u8 = 55;
const MAX_QUALIFYING_PER_STORE: usize = 8;

// ── Tauri commands ───────────────────────────────────────────────────────────

#[tauri::command]
pub async fn find_categories_for_story(app: AppHandle, request: FindCategoriesRequest) -> GenreResult {
    let database = app.state::<db::Db>();

    let genre_data = { let conn = database.0.lock().unwrap(); db::load_genre_data(&conn, &request.folder) };
    let genre_data = match genre_data {
        Some(d) => d,
        None    => return err("No genre data found. Run Analyze first."),
    };

    if request.canopy_api_key.is_empty() {
        return err("No Canopy API key set. Configure it in Settings.");
    }

    emit(&app, "Running Category Finder via Canopy API...");
    emit(&app, &format!("  Store: {}", request.store));

    // Get ranked genres
    let genre_terms: Vec<(String, u8)> = {
        let conn = database.0.lock().unwrap();
        db::get_genre_rankings(&conn, &request.folder, &request.store)
            .unwrap_or_default()
            .into_iter()
            .map(|r| (r.genre, r.confidence as u8))
            .collect()
    };

    if genre_terms.is_empty() {
        return err("No genre rankings found. Run Analyze first.");
    }

    let base_description = format!(
        "{}\n\nKDP category paths already identified: {}\n\n{}",
        genre_data.industry_ebook,
        genre_data.kdp_ebook.join("; "),
        genre_data.genre_signals
    );

    // Match categories from catalog
    let result = match_categories_by_store(&app, &database, &request.folder, &request.store, &base_description, &genre_terms, &request.provider, &request.api_key, &request.model).await;

    let final_cats = rank_by_discoverability(&app, &request.store, result.qualifying, &request.canopy_api_key).await;

    if final_cats.is_empty() {
        return GenreResult { success: true, report: "No candidates cleared the fit bar.".to_string(), error: String::new() };
    }

    // Store results
    {
        let conn = database.0.lock().unwrap();
        let report = format!("Found {} categories for {}.", final_cats.len(), request.store);
        let _ = db::save_document(&conn, &request.folder, "category_finder", &report);
    }

    emit(&app, &format!("✓ {} categories found and verified.", final_cats.len()));
    GenreResult { success: true, report: String::new(), error: String::new() }
}

#[tauri::command]
pub async fn match_categories_for_story(app: AppHandle, request: FindCategoriesRequest) -> GenreResult {
    let database = app.state::<db::Db>();

    let genre_data = { let conn = database.0.lock().unwrap(); db::load_genre_data(&conn, &request.folder) };
    let genre_data = match genre_data {
        Some(d) => d,
        None    => return err("No genre data found. Run Analyze first."),
    };

    let rankings = {
        let conn = database.0.lock().unwrap();
        db::get_genre_rankings(&conn, &request.folder, "Kindle").unwrap_or_default()
    };

    let genre_terms: Vec<(String, u8)> = if !rankings.is_empty() {
        rankings.iter().filter(|r| r.confidence >= 30).take(6)
            .map(|r| (r.genre.clone(), r.confidence as u8)).collect()
    } else {
        vec![(genre_data.industry_ebook.clone(), 100)]
    };

    let base_description = format!("{}\n\n{}", genre_data.industry_ebook, genre_data.genre_signals);
    emit(&app, &format!("Comparing {} genre(s) against the imported catalog — Kindle eBook and Paperback, one genre at a time:", genre_terms.len()));

    let stores = ["Kindle", "Books"];
    let mut store_sections: Vec<String> = Vec::new();
    let mut any_data = false;

    for store in stores {
        let total_catalog = { let conn = database.0.lock().unwrap(); db::kdp_category_count(&conn, store) };
        if total_catalog < 50 {
            emit(&app, &format!("⚠ Skipping {} — catalog nearly empty for this store.", store));
            store_sections.push(format!("## {}\n\n*Catalog nearly empty for this store — import WinningCat data covering {} to enable matching here, or use Find Categories (PR) instead.*\n", store, store));
            continue;
        }
        emit(&app, &format!("=== {} ===", store));

        let result = match_categories_by_store(&app, &database, &request.folder, store, &base_description, &genre_terms, &request.provider, &request.api_key, &request.model).await;

        let final_cats = rank_by_discoverability(&app, store, result.qualifying, &request.canopy_api_key).await;
        if !final_cats.is_empty() { any_data = true; }

        store_sections.push(render_store_match_section(store, &result.per_genre, &final_cats));
    }

    if !any_data {
        return err("No catalog categories cleared the fit bar for either store. Run Rank Genres first, import more WinningCat coverage, or use Find Categories to discover new paths.");
    }

    let report = serde_json::json!({
        "schema": "category_finder_v1",
        "method": "Only categories that honestly fit (confidence ≥55%, or independently found by 2+ ranked genres) are eligible. Every eligible candidate is verified live via Canopy API; final 3 ranked by real discoverability.",
        "stores": store_sections.iter().map(|s| serde_json::from_str::<serde_json::Value>(s).unwrap_or_default()).collect::<Vec<_>>(),
    }).to_string();

    { let conn = database.0.lock().unwrap(); let _ = db::save_document(&conn, &request.folder, "category_finder", &report); }
    emit(&app, "✓ Best-for-discoverability catalog match (both stores) saved to database.");

    GenreResult { success: true, report, error: String::new() }
}

#[tauri::command]
pub async fn verify_mapped_categories(app: AppHandle, db: tauri::State<'_, db::Db>, request: VerifyMappedRequest) -> Result<GenreResult, ()> {
    let canopy_key = {
        // The canopy key is stored in localStorage on the frontend — but for this
        // command we need it passed in or loaded. For now, check if it's empty.
        String::new()
    };

    let rankings = {
        let conn = db.0.lock().unwrap();
        match db::get_genre_rankings(&conn, &request.folder, &request.store) {
            Ok(r) => r,
            Err(e) => return Ok(err(&format!("Could not read rankings from database: {}", e))),
        }
    };

    if rankings.is_empty() {
        return Ok(err("No genre rankings found for this story. Run Rank Genres first."));
    }

    let mut paths: Vec<String> = Vec::new();
    for g in &rankings {
        for p in &g.kdp_paths {
            if !paths.contains(p) { paths.push(p.clone()); }
        }
    }

    if paths.is_empty() {
        return Ok(err("None of this story's ranked genres have a mapped KDP path yet. Run Category Finder to discover paths."));
    }

    emit(&app, &format!("Verifying {} mapped KDP path(s) via Canopy API...", paths.len()));

    let result = crate::canopy::analyze_categories_canopy(
        app.clone(),
        paths,
        request.store.clone(),
        canopy_key,
    ).await;

    if !result.success {
        return Ok(err(&result.error));
    }

    {
        let conn = db.0.lock().unwrap();
        let md = format!("Verified {} categories for {}.", result.rows.len(), request.store);
        let _ = db::save_document(&conn, &request.folder, "mapped_categories", &md);
        for g in &rankings {
            for p in &g.kdp_paths {
                let _ = db::upsert_kdp_path(&conn, &g.genre, p, &request.store, "category_analyzer", true);
            }
        }
    }

    emit(&app, "✓ Verified paths saved to database.");

    Ok(GenreResult { success: true, report: String::new(), error: String::new() })
}

// ── Core logic ───────────────────────────────────────────────────────────────

/// Takes fit-qualified candidates (already gated on honest fit — this
/// function does not judge fit, only discoverability) and checks all of them
/// live via Canopy API in one batched call, then ranks by real
/// discoverability.
pub(crate) async fn rank_by_discoverability(app: &AppHandle, store: &str, qualifying: Vec<(String, u8, Vec<String>)>, canopy_api_key: &str) -> Vec<QualifiedCategory> {
    if qualifying.is_empty() { return Vec::new(); }

    emit(app, &format!("  {} candidate(s) honestly fit {} — checking via Canopy API for discoverability...", qualifying.len(), store));

    let paths: Vec<String> = qualifying.iter().map(|(p, _, _)| p.clone()).collect();

    if canopy_api_key.is_empty() {
        emit(app, "    ⚠ No Canopy API key set. Configure it in Settings.");
        return qualifying.into_iter().map(|(path, fit_confidence, agreeing_genres)| {
            QualifiedCategory { path, fit_confidence, agreeing_genres, verified: false, sales_to_one: String::new(), sales_to_ten: String::new(), publisher_pct: String::new(), ku_pct: String::new(), discoverability_score: -1, top_books: Vec::new() }
        }).collect();
    }

    let result = crate::canopy::analyze_categories_canopy(
        app.clone(), paths, store.to_string(), canopy_api_key.to_string(),
    ).await;

    if !result.success {
        emit(app, &format!("    ⚠ Could not fetch live stats ({}) — falling back to fit-confidence order.", result.error));
    }

    let mut scored: Vec<QualifiedCategory> = qualifying.into_iter().map(|(path, fit_confidence, agreeing_genres)| {
        let stat = if result.success { result.rows.iter().find(|r| r.requested_path == path) } else { None };
        let (verified, sales_to_one, sales_to_ten, publisher_pct, ku_pct, top_books) = match stat {
            Some(r) if r.found => (true, r.sales_to_one.clone(), r.sales_to_ten.clone(), r.publisher_pct.clone(), r.ku_pct.clone(), r.top_books.clone()),
            _ => (false, String::new(), String::new(), String::new(), String::new(), Vec::new()),
        };
        let discoverability_score = compute_discoverability_score(verified, &sales_to_ten);
        QualifiedCategory { path, fit_confidence, agreeing_genres, verified, sales_to_one, sales_to_ten, publisher_pct, ku_pct, discoverability_score, top_books }
    }).collect();

    scored.sort_by(|a, b| {
        let sc = b.discoverability_score.cmp(&a.discoverability_score);
        if sc != std::cmp::Ordering::Equal { return sc; }
        b.fit_confidence.cmp(&a.fit_confidence)
    });

    for q in &scored {
        let stat_note = if q.verified { format!("sales to #10: {}", q.sales_to_ten) } else { "could not verify live".to_string() };
        emit(app, &format!("    `{}` — fit {}%, {}", q.path, q.fit_confidence, stat_note));
    }

    scored
}

/// Higher is more discoverable. Unverified (couldn't confirm live) always
/// loses to anything confirmed, regardless of fit.
fn compute_discoverability_score(verified: bool, sales_to_ten: &str) -> i32 {
    if !verified { return -1; }
    let n: Option<f64> = sales_to_ten.chars()
        .filter(|c| c.is_ascii_digit() || *c == '.')
        .collect::<String>().parse().ok();
    match n {
        Some(v) if (5.0..=30.0).contains(&v) => 3,  // sweet spot for a debut launch
        Some(v) if (3.0..60.0).contains(&v)  => 2,  // moderate
        Some(_)                               => 1,  // highly competitive or near-dead
        None                                  => 0,
    }
}

/// Match one store's catalog against every ranked genre, genre by genre.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn match_categories_by_store(
    app: &AppHandle, database: &db::Db, folder: &str, store: &str,
    base_description: &str, genre_terms: &[(String, u8)],
    provider: &str, api_key: &str, model: &str,
) -> StoreMatchResult {
    let mut per_genre: Vec<(String, u8, Vec<(String, u8, String)>)> = Vec::new();
    let mut path_genres: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    let mut path_conf: std::collections::HashMap<String, u8> = std::collections::HashMap::new();

    for (genre_name, genre_conf) in genre_terms {
        emit(app, &format!("  → {} ({}%)", genre_name, genre_conf));

        let candidates = {
            let conn = database.0.lock().unwrap();
            db::search_kdp_categories(&conn, store, std::slice::from_ref(genre_name), 100)
        };

        if candidates.is_empty() {
            emit(app, "      no catalog matches for this genre yet.");
            per_genre.push((genre_name.clone(), *genre_conf, Vec::new()));
            continue;
        }
        emit(app, &format!("      {} candidates found.", candidates.len()));

        let desc = format!("{}\n\nScore specifically against this one genre: {}", base_description, genre_name);
        let picks = match ai_match_from_catalog(provider, api_key, model, &desc, &candidates, 2).await {
            Ok(p) => p,
            Err(e) => { emit(app, &format!("      ⚠ AI error for this genre: {}", e)); Vec::new() }
        };
        if picks.is_empty() { emit(app, "      no confident match for this genre."); }
        for (path, conf, _reason) in &picks {
            emit(app, &format!("      {}% — {}", conf, path));
            path_genres.entry(path.clone()).or_default().push(genre_name.clone());
            path_conf.entry(path.clone()).or_insert(*conf);
        }
        per_genre.push((genre_name.clone(), *genre_conf, picks));
    }

    let mut ranked_paths: Vec<String> = path_genres.keys().cloned().collect();
    ranked_paths.sort_by(|a, b| {
        let agree = path_genres[b].len().cmp(&path_genres[a].len());
        if agree != std::cmp::Ordering::Equal { return agree; }
        path_conf[b].cmp(&path_conf[a])
    });

    // Persist every discovered candidate
    if !ranked_paths.is_empty() {
        let conn = database.0.lock().unwrap();
        let rows: Vec<(String, u8, String, String, String, String, String, Option<String>)> = ranked_paths.iter().map(|path| {
            let conf = path_conf[path];
            (path.clone(), conf, String::new(), String::new(), String::new(), String::new(), "matched".to_string(), None)
        }).collect();
        let top_genre = genre_terms.first().map(|(g, _)| g.clone());
        let _ = db::replace_category_results(&conn, folder, store, top_genre.as_deref(), &rows);
    }

    emit(app, &format!(
        "  Fit gate (confidence ≥{}%, or 2+ genres agree): {} of {} candidates qualify for a discoverability check.",
        FIT_CONFIDENCE_BAR, ranked_paths.iter().filter(|p| path_conf[*p] >= FIT_CONFIDENCE_BAR || path_genres[*p].len() >= 2).count(),
        ranked_paths.len()
    ));

    let qualifying: Vec<(String, u8, Vec<String>)> = ranked_paths.into_iter()
        .filter(|p| path_conf[p] >= FIT_CONFIDENCE_BAR || path_genres[p].len() >= 2)
        .take(MAX_QUALIFYING_PER_STORE)
        .map(|path| {
            let conf = path_conf[&path];
            let genres = path_genres[&path].clone();
            (path, conf, genres)
        }).collect();

    StoreMatchResult { per_genre, qualifying }
}

pub(crate) fn render_store_match_section(store: &str, per_genre: &[(String, u8, Vec<(String, u8, String)>)], final_cats: &[QualifiedCategory]) -> String {
    let json = serde_json::json!({
        "store": store,
        "per_genre": per_genre.iter().map(|(genre_name, genre_conf, picks)| {
            serde_json::json!({
                "genre": genre_name,
                "confidence": genre_conf,
                "picks": picks.iter().map(|(path, conf, reason)| {
                    serde_json::json!({ "path": path, "confidence": conf, "reason": reason })
                }).collect::<Vec<_>>(),
            })
        }).collect::<Vec<_>>(),
        "final_categories": final_cats.iter().enumerate().map(|(i, q)| {
            serde_json::json!({
                "rank": i + 1,
                "path": q.path,
                "fit_confidence": q.fit_confidence,
                "agreeing_genres": q.agreeing_genres,
                "verified": q.verified,
                "sales_to_one": q.sales_to_one,
                "sales_to_ten": q.sales_to_ten,
                "publisher_pct": q.publisher_pct,
                "ku_pct": q.ku_pct,
                "discoverability_score": q.discoverability_score,
                "is_bonus": i >= 3,
            })
        }).collect::<Vec<_>>(),
    });
    json.to_string()
}

pub(crate) async fn ai_match_from_catalog(provider: &str, api_key: &str, model: &str, description: &str, candidates: &[(String, String)], max_picks: usize)
    -> Result<Vec<(String, u8, String)>, String>
{
    let numbered = candidates.iter().enumerate()
        .map(|(i, (path, _))| format!("{}. {}", i + 1, path))
        .collect::<Vec<_>>()
        .join("\n");

    let system = format!(
        r#"You are an Amazon KDP category expert. Below is a REAL list of existing Amazon category paths for this book's store, pulled directly from Amazon's own category tree — not a guess. Pick the 1 to {} best-fitting categories for this book from ONLY this list.

Return ONLY a JSON array, no markdown, no preamble.
Each item: {{ "index": <1-based row number from the list>, "confidence": <0-100>, "reason": "<one sentence>" }}
Only include categories that are genuinely strong fits — an empty array is a valid answer if nothing fits well.
Sort descending by confidence.

Categories:
{}"#,
        max_picks, numbered
    );

    let raw = call_llm(provider, api_key, model, &system, description, 500).await?;
    let clean = raw.trim()
        .trim_start_matches("```json").trim_start_matches("```")
        .trim_end_matches("```").trim();

    let matches: Vec<CatalogMatch> = serde_json::from_str(clean)
        .or_else(|_| {
            // Some models return a single object instead of an array — wrap it
            serde_json::from_str::<CatalogMatch>(clean).map(|m| vec![m])
        })
        .map_err(|e| format!("Parse error (catalog match): {} | got: {}", e, &clean[..clean.len().min(300)]))?;

    Ok(matches.into_iter().filter_map(|m| {
        candidates.get(m.index.checked_sub(1)?).map(|(path, _)| (path.clone(), m.confidence, m.reason))
    }).collect())
}
