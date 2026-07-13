// genre_analyzer.rs — Full manuscript analysis pipeline
//
// Three commands:
//   generate_summaries  — Phase 1 only: chapter by chapter, saves to chapter_summaries table
//   analyze_genre       — Phase 2 only: reads summaries, produces genre_data row + rendered doc
//   run_full_analysis   — Phase 1 + 2: summaries + genre report
//                         PR competition data is handled separately by Analyze Competition
//
// Everything the pipeline produces (chapter summaries, genre classification,
// KDP keywords, PR search terms, category rankings/results) lives in SQLite
// (see db.rs). The .md reports shown in the Reports panel are rendered fresh
// from that data on every request via db::get_report_cmd — nothing is cached
// as a loose file that can go stale.

use std::path::{Path, PathBuf};
use std::fs;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::DialogExt;

use crate::commands::call_llm;
use crate::db;
use crate::keyword_search::KeywordResult;

// ── Shared result / request types ───────────────────────────────────────────

#[derive(Serialize)]
pub struct GenreResult {
    pub success: bool,
    pub report:  String,
    pub error:   String,
}

#[derive(Deserialize)]
pub struct FolderRequest {
    pub folder:   String,
    pub api_key:  String,
    pub model:    String,
    pub provider: String,
    #[serde(default)]
    pub canopy_api_key: String,
}

#[derive(Deserialize)]
pub struct AnalyzeStoryRequest {
    pub folder:            String,
    pub provider:          String,
    pub api_key:           String,
    pub model:             String,
    pub force_resummarize: bool,
    #[serde(default)]
    pub canopy_api_key:    String,
}

// ── Folder picker ────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn pick_manuscript_folder(app: AppHandle) -> Result<String, String> {
    use tauri_plugin_dialog::FilePath;
    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog()
        .file()
        .set_title("Select Manuscript Folder")
        .pick_folder(move |result| { let _ = tx.send(result); });
    match rx.recv() {
        Ok(Some(FilePath::Path(p))) => Ok(p.to_string_lossy().to_string()),
        Ok(_) => Err("No folder selected".to_string()),
        Err(e) => Err(e.to_string()),
    }
}

// ── Phase 1: chapter summaries ───────────────────────────────────────────────

#[tauri::command]
pub async fn generate_summaries(app: AppHandle, request: FolderRequest) -> GenreResult {
    tokio::task::spawn_blocking(move || {
        let folder = PathBuf::from(&request.folder);
        if !folder.exists() { return err("Folder does not exist."); }

        crate::reset_cancel();
        let chapters = collect_chapters(&folder);
        if chapters.is_empty() { return err("No .md files found."); }

        emit(&app, &format!("Found {} chapter file(s). Starting summaries...", chapters.len()));

        let database = app.state::<db::Db>();
        let (done, skipped) = phase1_summaries(&app, &database, &chapters, &request.folder, &request.provider, &request.api_key, &request.model);

        GenreResult {
            success: true,
            report:  format!("✓ {} summarized, {} already done.", done, skipped),
            error:   String::new(),
        }
    }).await.unwrap()
}

#[tauri::command]
pub async fn analyze_genre(app: AppHandle, request: FolderRequest) -> GenreResult {
    tokio::task::spawn_blocking(move || {
        let folder = PathBuf::from(&request.folder);
        if !folder.exists() { return err("Folder does not exist."); }

        let database = app.state::<db::Db>();
        let mut summaries = { let conn = database.0.lock().unwrap(); db::load_chapter_summaries(&conn, &request.folder) };

        if summaries.is_empty() {
            emit(&app, "No summaries found — running Phase 1 first...");
            let chapters = collect_chapters(&folder);
            if chapters.is_empty() { return err("No .md files found."); }
            phase1_summaries(&app, &database, &chapters, &request.folder, &request.provider, &request.api_key, &request.model);
            let conn = database.0.lock().unwrap();
            summaries = db::load_chapter_summaries(&conn, &request.folder);
        }

        if summaries.is_empty() { return err("Could not produce any chapter summaries."); }

        emit(&app, &format!("Phase 2: Analyzing {} chapter summaries...", summaries.len()));
        phase2_analyze(&app, &database, &request.folder, &summaries, &request.provider, &request.api_key, &request.model)
    }).await.unwrap()
}

/// Run everything except folder selection and chapter summaries:
/// Analyze Genre → Full Analysis → Optimize Keywords → Generate PR Keywords
#[tauri::command]
pub async fn run_everything(app: AppHandle, request: FolderRequest) -> GenreResult {
    tokio::task::spawn_blocking(move || {
        let folder = PathBuf::from(&request.folder);
        if !folder.exists() { return err("Folder does not exist."); }

        crate::reset_cancel();
        let database = app.state::<db::Db>();

        // ── Step 1: Ensure summaries exist ────────────────────────────────────
        let mut summaries = { let conn = database.0.lock().unwrap(); db::load_chapter_summaries(&conn, &request.folder) };
        if summaries.is_empty() {
            emit(&app, "Step 1: No summaries found — generating now...");
            let chapters = collect_chapters(&folder);
            if chapters.is_empty() { return err("No .md chapter files found."); }
            phase1_summaries(&app, &database, &chapters, &request.folder, &request.provider, &request.api_key, &request.model);
            let conn = database.0.lock().unwrap();
            summaries = db::load_chapter_summaries(&conn, &request.folder);
            if summaries.is_empty() { return err("Could not produce chapter summaries."); }
        } else {
            emit(&app, &format!("Step 1: {} summaries found — skipping.", summaries.len()));
        }
        if crate::is_cancelled() { return err("Cancelled."); }

        // ── Step 2: Genre analysis ─────────────────────────────────────────────
        emit(&app, "Step 2: Running genre analysis...");
        let genre_result = phase2_analyze(&app, &database, &request.folder, &summaries, &request.provider, &request.api_key, &request.model);
        if !genre_result.success { return genre_result; }
        if crate::is_cancelled() { return err("Cancelled."); }

        // ── Step 3: Full report ────────────────────────────────────────────────
        emit(&app, "Step 3: Building full report...");
        let genre_data = { let conn = database.0.lock().unwrap(); db::load_genre_data(&conn, &request.folder) };
        let genre_data = match genre_data {
            Some(d) => d,
            None    => return err("genre_data missing after analysis."),
        };
        let full_report = render_full_report(&genre_data, false);
        { let conn = database.0.lock().unwrap(); let _ = db::save_document(&conn, &request.folder, "full_report", &full_report); }
        emit(&app, "  ✓ Full report saved to database.");
        if crate::is_cancelled() { return err("Cancelled."); }

        // ── Step 4: Optimize KDP keywords ─────────────────────────────────────
        emit(&app, "Step 4: Optimizing KDP keywords...");
        match call_keyword_optimizer(&request.provider, &request.api_key, &request.model, &genre_data, &genre_data.genre_signals) {
            Ok((entries, strategy)) => {
                let conn = database.0.lock().unwrap();
                let _ = db::save_kdp_keywords(&conn, &request.folder, &entries, &strategy, "*(Generated from genre analysis.)*");
                let rendered = render_kdp_keywords(&entries, &strategy, "*(Generated from genre analysis.)*");
                let _ = db::save_document(&conn, &request.folder, "kdp_keywords", &rendered);
                emit(&app, "  ✓ KDP keywords saved to database.");
            }
            Err(e) => emit(&app, &format!("  ⚠ Keyword optimization failed: {}", e)),
        }
        if crate::is_cancelled() { return err("Cancelled."); }

        // ── Step 5: Generate PR keywords ──────────────────────────────────────
        emit(&app, "Step 5: Generating PR Competition Analyzer keywords...");
        let pr_system = r#"You are a Publisher Rocket expert. Generate short search phrases for the Competition Analyzer tool.

Rules:
- 2-4 words maximum per phrase
- Plain English, no special characters
- Think like a reader browsing Amazon
- Include: genre combinations, setting descriptors, theme words, reader mood phrases

Return ONLY a JSON array of strings. No markdown, no preamble. Example:
["christian historical fiction", "first century rome", "faith romance clean"]"#;

        let pr_user = format!(
            "Book genre: {}\nKDP categories: {}\nGenre signals:\n{}",
            genre_data.industry_ebook,
            genre_data.kdp_ebook.iter()
                .map(|p| p.split('>').last().unwrap_or(p).trim().to_string())
                .collect::<Vec<_>>().join(", "),
            &genre_data.genre_signals[..genre_data.genre_signals.len().min(500)]
        );

        match call_llm(&request.provider, &request.api_key, &request.model, pr_system, &pr_user, 300) {
            Ok(raw) => {
                if let Some(clean) = extract_json_object(&raw) {
                    if let Ok(keywords) = serde_json::from_str::<Vec<String>>(&clean) {
                        let conn = database.0.lock().unwrap();
                        let _ = db::save_pr_keywords(&conn, &request.folder, &keywords);
                        let rendered = render_pr_keywords(&keywords);
                        let _ = db::save_document(&conn, &request.folder, "pr_keywords", &rendered);
                        emit(&app, &format!("  ✓ {} PR search terms saved to database.", keywords.len()));
                        for kw in &keywords { emit(&app, &format!("    • {}", kw)); }
                    }
                } else {
                    emit(&app, "  ⚠ Could not parse PR keywords response.");
                }
            }
            Err(e) => emit(&app, &format!("  ⚠ PR keywords failed: {}", e)),
        }

        emit(&app, "✓ Analysis complete. Run Analyze Competition next.");

        GenreResult { success: true, report: full_report, error: String::new() }
    }).await.unwrap()
}

#[tauri::command]
pub async fn run_full_analysis(app: AppHandle, request: FolderRequest) -> GenreResult {
    tokio::task::spawn_blocking(move || {
        let folder = PathBuf::from(&request.folder);
        if !folder.exists() { return err("Folder does not exist."); }

        let database = app.state::<db::Db>();

        // ── Phase 1 ──────────────────────────────────────────────────────────
        let mut summaries = { let conn = database.0.lock().unwrap(); db::load_chapter_summaries(&conn, &request.folder) };
        if summaries.is_empty() {
            emit(&app, "Phase 1: Generating chapter summaries...");
            let chapters = collect_chapters(&folder);
            if chapters.is_empty() { return err("No .md files found."); }
            phase1_summaries(&app, &database, &chapters, &request.folder, &request.provider, &request.api_key, &request.model);
            let conn = database.0.lock().unwrap();
            summaries = db::load_chapter_summaries(&conn, &request.folder);
        } else {
            emit(&app, &format!("Phase 1: {} summaries already exist — skipping.", summaries.len()));
        }
        if summaries.is_empty() { return err("No chapter summaries available."); }

        // ── Phase 2 ──────────────────────────────────────────────────────────
        let existing = { let conn = database.0.lock().unwrap(); db::load_genre_data(&conn, &request.folder) };
        let genre_data = if let Some(d) = existing {
            emit(&app, "Phase 2: genre data exists in database — loading...");
            d
        } else {
            emit(&app, "Phase 2: Running genre analysis...");
            let r = phase2_analyze(&app, &database, &request.folder, &summaries, &request.provider, &request.api_key, &request.model);
            if !r.success { return r; }
            let conn = database.0.lock().unwrap();
            match db::load_genre_data(&conn, &request.folder) {
                Some(d) => d,
                None    => return err("Phase 2 produced no genre data."),
            }
        };

        emit(&app, &format!("  KDP ebook paths: {}", genre_data.kdp_ebook.join(", ")));
        emit(&app, &format!("  KDP print paths: {}", genre_data.kdp_print.join(", ")));

        // ── Build full report ─────────────────────────────────────────────────
        emit(&app, "Building full report...");
        let full_report = render_full_report(&genre_data, true);
        { let conn = database.0.lock().unwrap(); let _ = db::save_document(&conn, &request.folder, "full_report", &full_report); }
        emit(&app, "✓ Full report saved to database.");

        GenreResult { success: true, report: full_report, error: String::new() }
    }).await.unwrap()
}

// ── Analysis state check ──────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct AnalysisState {
    pub has_folder:                 bool,
    pub summary_count:              usize,
    pub has_genre_data:             bool,
    pub has_full_report:            bool,
    pub has_keywords:               bool,
    pub has_pr_keywords:            bool,
    pub has_competition:            bool,
    pub has_categories:             bool,
    pub has_genre_ranking:          bool,
    pub has_mapped_verified:        bool,
    pub has_bisac:                  bool,
    pub has_discovery_keywords:     bool,
    pub has_keyword_search_results: bool,
}

#[tauri::command]
pub async fn check_analysis_state(app: AppHandle, folder: String) -> AnalysisState {
    tokio::task::spawn_blocking(move || {
        let folder_path = PathBuf::from(&folder);
        let database    = app.state::<db::Db>();
        let conn        = database.0.lock().unwrap();

        AnalysisState {
            has_folder:                 folder_path.exists(),
            summary_count:              db::chapter_summary_count(&conn, &folder) as usize,
            has_genre_data:             db::load_genre_data(&conn, &folder).is_some(),
            has_full_report:            db::get_document(&conn, &folder, "full_report").is_some(),
            has_keywords:               db::load_kdp_keywords(&conn, &folder).is_some(),
            has_pr_keywords:            !db::load_pr_keywords(&conn, &folder).is_empty(),
            has_competition:            db::get_document(&conn, &folder, "competition_report").is_some(),
            has_categories:             db::has_category_results(&conn, &folder),
            has_genre_ranking:          db::has_genre_rankings(&conn, &folder),
            has_mapped_verified:        db::get_document(&conn, &folder, "mapped_categories").is_some(),
            has_bisac:                  db::has_bisac_classifications(&conn, &folder),
            has_discovery_keywords:     !db::load_discovery_keywords(&conn, &folder).is_empty(),
            has_keyword_search_results: db::has_keyword_search_results(&conn, &folder),
        }
    }).await.unwrap()
}

// ── Category Finder (wired to Analyzer panel) ─────────────────────────────────
//
// Runs Publisher Rocket's Category Search against the book's genre data.
// The store (Kindle vs Books) is the only thing the user chooses here — the
// filter is always locked to "Selectable Excluding Ghosts", since ghost
// (unselectable) categories are never useful for KDP category assignment.

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

#[tauri::command]
pub async fn find_categories_for_story(app: AppHandle, request: FindCategoriesRequest) -> GenreResult {
    tokio::task::spawn_blocking(move || {
        let database = app.state::<db::Db>();

        let genre_data = { let conn = database.0.lock().unwrap(); db::load_genre_data(&conn, &request.folder) };
        let genre_data = match genre_data {
            Some(d) => d,
            None    => return err("No genre data found. Run Analyze first."),
        };

        let genre_description = format!(
            "{}\n\nKDP category paths already identified from manuscript analysis: {}\n\n{}",
            genre_data.industry_ebook,
            genre_data.kdp_ebook.join("; "),
            genre_data.genre_signals
        );

        emit(&app, "Running Category Finder on Publisher Rocket...");
        emit(&app, &format!("  Store: {}", request.store));
        emit(&app, "  Filter: Selectable Excluding Ghosts");

        use crate::category_finder;
        match category_finder::find_categories(
            &app,
            &genre_description,
            &request.store,
            "Selectable Excluding Ghosts",
            &request.provider,
            &request.api_key,
            &request.model,
        ) {
            Err(e) => err(&e),
            Ok(results) => {
                let now = chrono::Utc::now().format("%B %-d, %Y %H:%M UTC").to_string();
                let mut lines = vec![
                    "# Category Finder Results".to_string(),
                    format!("Generated: {}", now),
                    format!("Store: {}", request.store),
                    "Filter: Selectable Excluding Ghosts".to_string(),
                    String::new(),
                ];

                let high: Vec<_> = results.iter().filter(|r| r.note.is_none() && r.confidence >= 80).collect();
                let low:  Vec<_> = results.iter().filter(|r| r.note.is_none() && r.confidence <  80).collect();
                let failed: Vec<_> = results.iter().filter(|r| r.note.is_some()).collect();

                if !high.is_empty() {
                    lines.push("## Matched Categories".to_string());
                    lines.push(String::new());
                    for r in &high {
                        lines.push(format!("### {} ({}% match)", r.path, r.confidence));
                        lines.push(String::new());
                        if !r.stats.is_empty() {
                            lines.push(format!("- Sales needed to reach #1: **{}**", r.stats.sales_to_one));
                            lines.push(format!("- Sales needed to reach #10: **{}**", r.stats.sales_to_ten));
                            lines.push(format!("- Publisher books: **{}**", r.stats.publisher_pct));
                            lines.push(format!("- KU books: **{}**", r.stats.ku_pct));
                            lines.push(String::new());
                        }
                        if !r.keywords.is_empty() {
                            lines.push("**Keywords**".to_string());
                            lines.push(String::new());
                            lines.push(r.keywords.clone());
                            lines.push(String::new());
                        } else {
                            lines.push("*⚠️ Keywords could not be scraped for this category.*".to_string());
                            lines.push(String::new());
                        }
                        lines.push("---".to_string());
                        lines.push(String::new());
                    }
                } else {
                    lines.push("## No High-Confidence Match Found".to_string());
                    lines.push(String::new());
                    lines.push("Use one of these paths in the Category Analyzer to inspect manually.".to_string());
                    lines.push(String::new());
                }

                if !low.is_empty() {
                    lines.push("## Also Considered (below 80%)".to_string());
                    lines.push(String::new());
                    for (i, r) in low.iter().enumerate() {
                        lines.push(format!("{}. {} — **{}%**", i + 1, r.path, r.confidence));
                    }
                    lines.push(String::new());
                }

                if !failed.is_empty() {
                    lines.push("## Search Failed — Check Manually".to_string());
                    lines.push(String::new());
                    lines.push("Publisher Rocket didn't return usable data for these — not a confidence judgment, just a scrape that didn't land. Check them by hand in Category Search.".to_string());
                    lines.push(String::new());
                    for r in &failed {
                        lines.push(format!("- **{}** — {}", r.path, r.note.as_deref().unwrap_or("")));
                    }
                    lines.push(String::new());
                }

                {
                    let conn = database.0.lock().unwrap();
                    let top_genre_hint: Option<String> = db::get_genre_rankings(&conn, &request.folder, &request.store)
                        .ok()
                        .and_then(|rows| rows.into_iter().next())
                        .map(|r| r.genre);

                    let rows: Vec<(String, u8, String, String, String, String, String, String, Option<String>)> = results.iter()
                        .map(|r| {
                            let status = if r.note.is_some() { "failed" }
                                else if r.confidence >= 80 { "matched" }
                                else { "considered" };
                            (
                                r.path.clone(), r.confidence,
                                r.stats.sales_to_one.clone(), r.stats.sales_to_ten.clone(),
                                r.stats.publisher_pct.clone(), r.stats.ku_pct.clone(),
                                r.keywords.clone(), status.to_string(), r.note.clone(),
                            )
                        })
                        .collect();

                    if let Err(e) = db::replace_category_results(&conn, &request.folder, &request.store, top_genre_hint.as_deref(), &rows) {
                        emit(&app, &format!("  ⚠ Could not save results to database: {}", e));
                    }

                    let report = lines.join("\n");
                    let _ = db::save_document(&conn, &request.folder, "category_finder", &report);
                    emit(&app, &format!("✓ Results stored in database — {} matched, {} also considered, {} search failure(s).", high.len(), low.len(), failed.len()));

                    return GenreResult { success: true, report, error: String::new() };
                }
            }
        }
    }).await.unwrap()
}

// ── Match Categories (catalog only — no Publisher Rocket) ─────────────────
//
// Matches the story's ranked genres directly against the imported category
// catalog. Pure AI + database — no live Publisher Rocket call at all. The
// deliverable is genre-fit confidence and cross-genre agreement; live
// sales-rank numbers are something the user checks themselves on PR or the
// KDP site when they're actually ready to set metadata, not something this
// step needs to fetch or store.

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

        let app_c = app.clone();
        let folder_c = request.folder.clone();
        let store_c = store.to_string();
        let desc_c = base_description.clone();
        let terms_c = genre_terms.clone();
        let provider_c = request.provider.clone();
        let api_key_c = request.api_key.clone();
        let model_c = request.model.clone();

        let result = tokio::task::spawn_blocking(move || {
            let database = app_c.state::<db::Db>();
            match_categories_by_store(&app_c, &database, &folder_c, &store_c, &desc_c, &terms_c, &provider_c, &api_key_c, &model_c)
        }).await.unwrap();

        let final_cats = rank_by_discoverability(&app, store, result.qualifying, &request.canopy_api_key).await;
        if !final_cats.is_empty() { any_data = true; }

        store_sections.push(render_store_match_section(store, &result.per_genre, &final_cats));
    }

    if !any_data {
        return err("No catalog categories cleared the fit bar for either store. Run Rank Genres first, import more WinningCat coverage, or use Find Categories to discover new paths.");
    }

    let report = serde_json::json!({
        "schema": "category_finder_v1",
        "method": "Only categories that honestly fit (confidence ≥55%, or independently found by 2+ ranked genres) are eligible. Every eligible candidate is checked live in Publisher Rocket; final 3 ranked by real discoverability.",
        "stores": store_sections.iter().map(|s| serde_json::from_str::<serde_json::Value>(s).unwrap_or_default()).collect::<Vec<_>>(),
    }).to_string();

    { let conn = database.0.lock().unwrap(); let _ = db::save_document(&conn, &request.folder, "category_finder", &report); }
    emit(&app, "✓ Best-for-discoverability catalog match (both stores) saved to database.");

    GenreResult { success: true, report, error: String::new() }
}

/// A candidate that cleared the fit gate, with its live Publisher Rocket
/// discoverability data (if it could be confirmed) and computed score.
struct QualifiedCategory {
    path:                   String,
    fit_confidence:         u8,
    agreeing_genres:        Vec<String>,
    verified:               bool,
    sales_to_one:           String,
    sales_to_ten:           String,
    publisher_pct:          String,
    ku_pct:                 String,
    discoverability_score:  i32,
}

/// Takes fit-qualified candidates (already gated on honest fit — this
/// function does not judge fit, only discoverability) and checks all of them
/// live in Publisher Rocket in one batched call, then ranks by real
/// discoverability. This is the actual method for "best discoverability
/// among what honestly fits" — not a narrow tiebreak, a real ranking over
/// every candidate that earned a look.
async fn rank_by_discoverability(app: &AppHandle, store: &str, qualifying: Vec<(String, u8, Vec<String>)>, canopy_api_key: &str) -> Vec<QualifiedCategory> {
    if qualifying.is_empty() { return Vec::new(); }

    emit(app, &format!("  {} candidate(s) honestly fit {} — checking via Canopy API for discoverability...", qualifying.len(), store));

    let paths: Vec<String> = qualifying.iter().map(|(p, _, _)| p.clone()).collect();

    if canopy_api_key.is_empty() {
        emit(app, "    ⚠ No Canopy API key set. Configure it in Settings.");
        return qualifying.into_iter().map(|(path, fit_confidence, agreeing_genres)| {
            QualifiedCategory { path, fit_confidence, agreeing_genres, verified: false, sales_to_one: String::new(), sales_to_ten: String::new(), publisher_pct: String::new(), ku_pct: String::new(), discoverability_score: -1 }
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
        let (verified, sales_to_one, sales_to_ten, publisher_pct, ku_pct) = match stat {
            Some(r) if r.found => (true, r.sales_to_one.clone(), r.sales_to_ten.clone(), r.publisher_pct.clone(), r.ku_pct.clone()),
            _ => (false, String::new(), String::new(), String::new(), String::new()),
        };
        let discoverability_score = compute_discoverability_score(verified, &sales_to_ten);
        QualifiedCategory { path, fit_confidence, agreeing_genres, verified, sales_to_one, sales_to_ten, publisher_pct, ku_pct, discoverability_score }
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
/// loses to anything confirmed, regardless of fit — an unconfirmed category
/// can't honestly be called "best for discoverability."
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
/// Persists every candidate discovered to category_results (not just the
/// fit-qualified subset — that's still useful history), but returns only
/// the fit-gated `qualifying` list: a category only earns a live Publisher
/// Rocket check if it genuinely fits (confidence ≥55%, or independently
/// found by 2+ ranked genres). Fit is judged here; discoverability is judged
/// afterward by rank_by_discoverability, on this pre-filtered set only.
struct StoreMatchResult {
    per_genre:  Vec<(String, u8, Vec<(String, u8, String)>)>,  // genre, genre_conf, picks(path,conf,reason)
    qualifying: Vec<(String, u8, Vec<String>)>,                 // path, fit_confidence, agreeing genres
}

const FIT_CONFIDENCE_BAR: u8 = 55;
const MAX_QUALIFYING_PER_STORE: usize = 8;

#[allow(clippy::too_many_arguments)]
fn match_categories_by_store(
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
        let picks = match ai_match_from_catalog(provider, api_key, model, &desc, &candidates, 2) {
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

    // Persist every discovered candidate, not just the fit-qualified subset
    // — this is history/growth for the catalog, independent of what makes
    // this particular report's final 3.
    if !ranked_paths.is_empty() {
        let conn = database.0.lock().unwrap();
        let rows: Vec<(String, u8, String, String, String, String, String, String, Option<String>)> = ranked_paths.iter().map(|path| {
            let conf = path_conf[path];
            (path.clone(), conf, String::new(), String::new(), String::new(), String::new(), String::new(), "matched".to_string(), None)
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

fn render_store_match_section(store: &str, per_genre: &[(String, u8, Vec<(String, u8, String)>)], final_cats: &[QualifiedCategory]) -> String {
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

#[derive(Deserialize)]
struct CatalogMatch { index: usize, confidence: u8, reason: String }

fn ai_match_from_catalog(provider: &str, api_key: &str, model: &str, description: &str, candidates: &[(String, String)], max_picks: usize)
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

    let raw = call_llm(provider, api_key, model, &system, description, 500)?;
    let clean = raw.trim()
        .trim_start_matches("```json").trim_start_matches("```")
        .trim_end_matches("```").trim();

    let matches: Vec<CatalogMatch> = serde_json::from_str(clean)
        .map_err(|e| format!("Parse error (catalog match): {} | got: {}", e, &clean[..clean.len().min(300)]))?;

    Ok(matches.into_iter().filter_map(|m| {
        candidates.get(m.index.checked_sub(1)?).map(|(path, _)| (path.clone(), m.confidence, m.reason))
    }).collect())
}

// ── BISAC Classification ───────────────────────────────────────────────────────────
//
// BISAC is the actual industry-standard subject code system (maintained by
// BISG) submitted as metadata for KDP Print and any wide/Ingram distribution
// — a completely separate system from Amazon's browsable KDP categories.
// Convention is max 3 codes per book, primary first. The seed list in
// data/bisac-fiction.json needs spot-checking against BISG's official free
// lookup (bisg.org/complete-bisac-subject-headings-list) before submitting
// anywhere — flagged in the rendered report every time.

#[derive(Deserialize)]
struct AiBisacPick { code: String, confidence: u8, reason: String }

#[tauri::command]
pub async fn classify_bisac_for_story(app: AppHandle, request: FolderRequest) -> GenreResult {
    tokio::task::spawn_blocking(move || {
        let database = app.state::<db::Db>();

        let genre_data = { let conn = database.0.lock().unwrap(); db::load_genre_data(&conn, &request.folder) };
        let genre_data = match genre_data {
            Some(d) => d,
            None    => return err("No genre data found. Run Analyze first."),
        };

        let master_list = { let conn = database.0.lock().unwrap(); db::master_bisac_list(&conn) };
        if master_list.is_empty() { return err("No BISAC codes loaded in the database."); }

        emit(&app, "Classifying against BISAC subject headings...");
        emit(&app, &format!("  Scoring against {} known codes.", master_list.len()));

        let description = format!("{}\n\n{}", genre_data.industry_ebook, genre_data.genre_signals);

        match ai_pick_bisac(&request.provider, &request.api_key, &request.model, &description, &master_list) {
            Err(e) => err(&e),
            Ok(picks) => {
                if picks.is_empty() { return err("AI did not select any BISAC codes."); }

                for p in &picks { emit(&app, &format!("  {}% — {} {}", p.2, p.0, p.1)); }

                let now_disp = chrono::Utc::now().format("%B %-d, %Y %H:%M UTC").to_string();
                let mut lines = vec![
                    "# BISAC Classification".to_string(),
                    format!("Generated: {}", now_disp),
                    String::new(),
                    "> **Verify before use.** These codes are AI-selected from a hand-seeded reference list, not a live BISG feed. Spot-check every code against BISG's free lookup (bisg.org/complete-bisac-subject-headings-list) before submitting to KDP Print or IngramSpark.".to_string(),
                    String::new(),
                    "BISAC convention: use up to 3 codes, primary listed first. This is separate from your Amazon KDP browse categories — Kindle eBook no longer takes BISAC directly (Amazon derives it from your browse category), but KDP Print and Ingram still require it explicitly.".to_string(),
                    String::new(),
                    "**On discoverability:** unlike KDP categories, there is no live data source for BISAC — Publisher Rocket doesn't cover it, and Amazon has no browse mechanism for it. When two codes are close in fit, a more specific heading is preferred over a generic \"/ General\" one as a structural best-practice, not measured data.".to_string(),
                    String::new(),
                    "---".to_string(),
                    String::new(),
                ];
                for (i, (code, heading, confidence, reason)) in picks.iter().enumerate() {
                    lines.push(format!("## {}. `{}` — {}", i + 1, code, heading));
                    lines.push(String::new());
                    lines.push(format!("**{}% confidence** — {}", confidence, reason));
                    lines.push(String::new());
                }

                let report = lines.join("\n");

                let conn = database.0.lock().unwrap();
                let rows: Vec<(String, String, u8, String)> = picks.iter()
                    .map(|(code, heading, conf, reason)| (code.clone(), heading.clone(), *conf, reason.clone()))
                    .collect();
                if let Err(e) = db::replace_bisac_classifications(&conn, &request.folder, "ebook", &rows) {
                    emit(&app, &format!("  ⚠ Could not save BISAC classification to database: {}", e));
                }
                let _ = db::save_document(&conn, &request.folder, "bisac_classification", &report);
                emit(&app, &format!("✓ BISAC classification saved to database — {} code(s).", picks.len()));

                GenreResult { success: true, report, error: String::new() }
            }
        }
    }).await.unwrap()
}

fn ai_pick_bisac(provider: &str, api_key: &str, model: &str, description: &str, master_list: &[db::BisacCodeRow])
    -> Result<Vec<(String, String, u8, String)>, String>
{
    let listing = master_list.iter()
        .map(|c| format!("{} — {}", c.code, c.heading))
        .collect::<Vec<_>>()
        .join("\n");

    let system = format!(
        r#"You are a BISAC subject-code classification expert. Choose the BEST 1 to 3 BISAC codes for this book from ONLY the list below — never invent a code that isn't listed. List the single best (primary) code first.

Return ONLY a JSON array, no markdown, no preamble.
Each item: {{ "code": "<exact code from list>", "confidence": <0-100>, "reason": "<one sentence>" }}
Order primary-first (highest confidence first).

BISAC codes:
{}"#,
        listing
    );

    let raw = call_llm(provider, api_key, model, &system, description, 500)?;
    let clean = raw.trim()
        .trim_start_matches("```json").trim_start_matches("```")
        .trim_end_matches("```").trim();

    let picks: Vec<AiBisacPick> = serde_json::from_str(clean)
        .map_err(|e| format!("Parse error (BISAC): {} | got: {}", e, &clean[..clean.len().min(300)]))?;

    let resolved: Vec<(String, String, u8, String)> = picks.into_iter().filter_map(|p| {
        master_list.iter().find(|c| c.code == p.code)
            .map(|c| (c.code.clone(), c.heading.clone(), p.confidence, p.reason))
    }).collect();

    Ok(resort_bisac_for_specificity(resolved))
}

/// No live discoverability data exists for BISAC anywhere — Publisher
/// Rocket doesn't cover it, and there's no Amazon-style browse mechanism for
/// it. The best defensible, structural (not measured) signal: when two codes
/// are close in fit confidence, a more specific heading faces less crowding
/// within that metadata bucket than a catch-all "/ General" one. Only
/// re-orders genuinely close calls (within 5 points) — a clear fit winner
/// from the AI is never overridden by this heuristic.
fn resort_bisac_for_specificity(mut picks: Vec<(String, String, u8, String)>) -> Vec<(String, String, u8, String)> {
    picks.sort_by(|a, b| {
        let conf_diff = (b.2 as i32 - a.2 as i32).abs();
        if conf_diff <= 5 {
            let specific_a = !a.1.to_lowercase().trim_end().ends_with("general");
            let specific_b = !b.1.to_lowercase().trim_end().ends_with("general");
            if specific_a != specific_b { return specific_b.cmp(&specific_a); }
        }
        b.2.cmp(&a.2)
    });
    picks
}

// ── Find Genres & Categories (the one-button combined report) ────────────────
//
// Does everything needed to answer "what genres and categories does this
// book belong in": self-bootstraps summaries + genre analysis if missing,
// then Rank Genres, then Match Categories for both formats, then BISAC for
// ebook and print (print only shown separately if it actually differs from
// ebook), plus positioning context (reader demographic, shelving, comps).
// Reuses the exact same core functions as the standalone buttons — nothing
// here is a separate, drifting implementation.

#[tauri::command]
pub async fn find_genres_and_categories_for_story(app: AppHandle, request: FolderRequest) -> GenreResult {
    let database = app.state::<db::Db>();

    // ── Ensure genre_data exists ── self-bootstrap summaries + analysis if missing ──
    // Safe to auto-run: phase1_summaries only summarizes chapters that
    // don't already have one (chapter_summary_exists check per chapter),
    // so this never re-spends AI credits on a chapter already done.
    let mut genre_data = { let conn = database.0.lock().unwrap(); db::load_genre_data(&conn, &request.folder) };
    if genre_data.is_none() {
        emit(&app, "No genre data yet — running Analyze first...");
        let app_c = app.clone();
        let folder_c = request.folder.clone();
        let provider_c = request.provider.clone();
        let api_key_c = request.api_key.clone();
        let model_c = request.model.clone();

        let bootstrap: Result<(), String> = tokio::task::spawn_blocking(move || {
            let database = app_c.state::<db::Db>();
            let folder_path = PathBuf::from(&folder_c);
            if !folder_path.exists() { return Err("Folder does not exist.".to_string()); }

            let mut summaries = { let conn = database.0.lock().unwrap(); db::load_chapter_summaries(&conn, &folder_c) };
            if summaries.is_empty() {
                let chapters = collect_chapters(&folder_path);
                if chapters.is_empty() { return Err("No .md chapter files found.".to_string()); }
                phase1_summaries(&app_c, &database, &chapters, &folder_c, &provider_c, &api_key_c, &model_c);
                let conn = database.0.lock().unwrap();
                summaries = db::load_chapter_summaries(&conn, &folder_c);
            }
            if summaries.is_empty() { return Err("Could not produce chapter summaries.".to_string()); }

            let r = phase2_analyze(&app_c, &database, &folder_c, &summaries, &provider_c, &api_key_c, &model_c);
            if !r.success { return Err(r.error); }
            Ok(())
        }).await.unwrap();

        if let Err(e) = bootstrap { return err(&e); }
        genre_data = { let conn = database.0.lock().unwrap(); db::load_genre_data(&conn, &request.folder) };
    }
    let genre_data = match genre_data {
        Some(d) => d,
        None    => return err("Could not produce genre data."),
    };

    let mut report_sections: Vec<String> = Vec::new();

    // ── Rank Genres ────────────────────────────────────────────────────
    emit(&app, "Ranking manuscript against master genre list...");
    let ranked: Vec<RankedGenre> = {
        let app_c = app.clone();
        let folder_c = request.folder.clone();
        let provider_c = request.provider.clone();
        let api_key_c = request.api_key.clone();
        let model_c = request.model.clone();
        let industry_ebook = genre_data.industry_ebook.clone();
        let kdp_ebook_joined = genre_data.kdp_ebook.join("; ");
        let genre_signals = genre_data.genre_signals.clone();

        let res: Result<Vec<RankedGenre>, String> = tokio::task::spawn_blocking(move || -> Result<Vec<RankedGenre>, String> {
            let database = app_c.state::<db::Db>();
            let master_list = crate::genre_taxonomy::master_genre_list(&database)
                .map_err(|e| format!("Could not load genre list from database: {}", e))?;
            let ai_ranked = ai_rank_genres(
                &provider_c, &api_key_c, &model_c,
                &format!("{}\n\nKDP paths already identified: {}\n\n{}", industry_ebook, kdp_ebook_joined, genre_signals),
                &master_list,
            )?;
            let mut ranked: Vec<RankedGenre> = ai_ranked.into_iter().map(|r| {
                let kdp_paths = crate::genre_taxonomy::kdp_paths_for_genre(&database, &r.genre, "Kindle").unwrap_or_default();
                RankedGenre { genre: r.genre, confidence: r.confidence, reason: r.reason, kdp_paths }
            }).collect();
            ranked.sort_by(|a, b| b.confidence.cmp(&a.confidence));

            let conn = database.0.lock().unwrap();
            let rows: Vec<(String, u8, String)> = ranked.iter().map(|r| (r.genre.clone(), r.confidence, r.reason.clone())).collect();
            let _ = db::replace_genre_rankings(&conn, &folder_c, &rows);
            let genre_ranking_md = {
                let mut s = vec!["# Genre Ranking".to_string(), String::new()];
                for r in &ranked { s.push(format!("## {} — {}%", r.genre, r.confidence)); s.push(String::new()); s.push(r.reason.clone()); s.push(String::new()); }
                s.join("\n")
            };
            let _ = db::save_document(&conn, &folder_c, "genre_ranking", &genre_ranking_md);

            Ok(ranked)
        }).await.unwrap();

        match res {
            Ok(r) => r,
            Err(e) => return err(&format!("Genre ranking failed: {}", e)),
        }
    };
    for r in &ranked { emit(&app, &format!("  {}% — {}", r.confidence, r.genre)); }

    report_sections.push({
        let mut s = vec!["## Genre Ranking".to_string(), String::new(),
            "Scored independently — percentages do not sum to 100.".to_string(), String::new()];
        for r in &ranked { s.push(format!("- **{}** — {}%", r.genre, r.confidence)); }
        s.push(String::new());
        s.join("\n")
    });

    let genre_terms: Vec<(String, u8)> = if !ranked.is_empty() {
        ranked.iter().filter(|r| r.confidence >= 30).take(6).map(|r| (r.genre.clone(), r.confidence)).collect()
    } else {
        vec![(genre_data.industry_ebook.clone(), 100)]
    };

    // ── KDP Categories, both formats, with PR tiebreak for a close 3rd/4th slot ──
    emit(&app, "Matching KDP categories against the imported catalog...");
    let base_description = format!("{}\n\n{}", genre_data.industry_ebook, genre_data.genre_signals);
    let mut kdp_section = vec!["## KDP Categories".to_string(), String::new()];
    for (store, label) in [("Kindle", "Kindle eBook"), ("Books", "Paperback")] {
        kdp_section.push(format!("### {}", label));
        kdp_section.push(String::new());
        let total_catalog = { let conn = database.0.lock().unwrap(); db::kdp_category_count(&conn, store) };
        if total_catalog < 50 {
            kdp_section.push("*Catalog nearly empty for this store — import WinningCat data, or use Find Categories (PR).*".to_string());
            kdp_section.push(String::new());
            continue;
        }

        let app_c = app.clone();
        let folder_c = request.folder.clone();
        let store_c = store.to_string();
        let desc_c = base_description.clone();
        let terms_c = genre_terms.clone();
        let provider_c = request.provider.clone();
        let api_key_c = request.api_key.clone();
        let model_c = request.model.clone();

        let result = tokio::task::spawn_blocking(move || {
            let database = app_c.state::<db::Db>();
            match_categories_by_store(&app_c, &database, &folder_c, &store_c, &desc_c, &terms_c, &provider_c, &api_key_c, &model_c)
        }).await.unwrap();

        let final_cats = rank_by_discoverability(&app, store, result.qualifying, &request.canopy_api_key).await;

        if final_cats.is_empty() {
            kdp_section.push("*No candidates cleared the fit bar for this store.*".to_string());
        } else {
            for (i, q) in final_cats.iter().enumerate() {
                let bonus = if i >= 3 { " — bonus candidate for post-launch" } else { "" };
                let disc_note = if q.verified {
                    format!(" — sales to #10: {}", q.sales_to_ten)
                } else {
                    " — could not verify live".to_string()
                };
                kdp_section.push(format!("{}. `{}` (fit {}%){}{} — matched by: {}", i + 1, q.path, q.fit_confidence, bonus, disc_note, q.agreeing_genres.join(", ")));
            }
        }
        kdp_section.push(String::new());
    }
    report_sections.push(kdp_section.join("\n"));

    // ── BISAC, ebook then print if different ───────────────────────
    emit(&app, "Classifying BISAC subject headings...");
    let (ebook_picks, print_picks_opt): (Vec<(String, String, u8, String)>, Option<Vec<(String, String, u8, String)>>) = {
        let app_c = app.clone();
        let folder_c = request.folder.clone();
        let provider_c = request.provider.clone();
        let api_key_c = request.api_key.clone();
        let model_c = request.model.clone();
        let industry_ebook = genre_data.industry_ebook.clone();
        let industry_print = genre_data.industry_print.clone();
        let genre_signals = genre_data.genre_signals.clone();
        let same_as_ebook = industry_print.trim().eq_ignore_ascii_case(industry_ebook.trim());

        tokio::task::spawn_blocking(move || {
            let database = app_c.state::<db::Db>();
            let bisac_master = { let conn = database.0.lock().unwrap(); db::master_bisac_list(&conn) };

            let ebook_desc = format!("{}\n\n{}", industry_ebook, genre_signals);
            let ebook_picks = ai_pick_bisac(&provider_c, &api_key_c, &model_c, &ebook_desc, &bisac_master).unwrap_or_default();
            {
                let conn = database.0.lock().unwrap();
                let rows: Vec<(String, String, u8, String)> = ebook_picks.iter().map(|(c, h, cf, r)| (c.clone(), h.clone(), *cf, r.clone())).collect();
                let _ = db::replace_bisac_classifications(&conn, &folder_c, "ebook", &rows);
            }

            let print_picks_opt = if same_as_ebook {
                let conn = database.0.lock().unwrap();
                let rows: Vec<(String, String, u8, String)> = ebook_picks.iter().map(|(c, h, cf, r)| (c.clone(), h.clone(), *cf, r.clone())).collect();
                let _ = db::replace_bisac_classifications(&conn, &folder_c, "print", &rows);
                None
            } else {
                let print_desc = format!("{}\n\n{}", industry_print, genre_signals);
                let print_picks = ai_pick_bisac(&provider_c, &api_key_c, &model_c, &print_desc, &bisac_master).unwrap_or_default();
                let conn = database.0.lock().unwrap();
                let rows: Vec<(String, String, u8, String)> = print_picks.iter().map(|(c, h, cf, r)| (c.clone(), h.clone(), *cf, r.clone())).collect();
                let _ = db::replace_bisac_classifications(&conn, &folder_c, "print", &rows);
                Some(print_picks)
            };

            (ebook_picks, print_picks_opt)
        }).await.unwrap()
    };

    let mut bisac_section = vec![
        "## BISAC Classification".to_string(), String::new(),
        "*Verify against BISG's free lookup (bisg.org/complete-bisac-subject-headings-list) before submitting anywhere. Kindle eBook no longer takes BISAC directly on KDP; this matters for KDP Print and wide/Ingram distribution. No live discoverability data exists for BISAC — close calls are broken by preferring a specific heading over a generic \"/ General\" one, a structural heuristic, not measured data.*".to_string(),
        String::new(),
    ];

    bisac_section.push("### Ebook".to_string());
    bisac_section.push(String::new());
    if ebook_picks.is_empty() {
        bisac_section.push("*No confident BISAC match.*".to_string());
    } else {
        for (i, (code, heading, conf, _reason)) in ebook_picks.iter().enumerate() {
            bisac_section.push(format!("{}. `{}` — {} ({}%)", i + 1, code, heading, conf));
        }
    }
    bisac_section.push(String::new());

    bisac_section.push("### Print".to_string());
    bisac_section.push(String::new());
    match &print_picks_opt {
        None => bisac_section.push("*Same as ebook — print genre tag matches ebook.*".to_string()),
        Some(print_picks) => {
            let ebook_codes: std::collections::HashSet<String> = ebook_picks.iter().map(|(c, _, _, _)| c.clone()).collect();
            let print_codes: std::collections::HashSet<String> = print_picks.iter().map(|(c, _, _, _)| c.clone()).collect();
            if !print_picks.is_empty() && ebook_codes == print_codes {
                bisac_section.push("*Same codes as ebook.*".to_string());
            } else if print_picks.is_empty() {
                bisac_section.push("*No confident BISAC match.*".to_string());
            } else {
                for (i, (code, heading, conf, _reason)) in print_picks.iter().enumerate() {
                    bisac_section.push(format!("{}. `{}` — {} ({}%)", i + 1, code, heading, conf));
                }
            }
        }
    }
    bisac_section.push(String::new());
    report_sections.push(bisac_section.join("\n"));

    // ── Positioning context ── useful when actually filling out KDP metadata ──
    let mut context_section = vec!["## Positioning Context".to_string(), String::new()];
    context_section.push(format!("**Reader demographic:** {}", genre_data.reader_demographic));
    context_section.push(format!("**Bookstore shelving:** {}", genre_data.bookstore_shelving));
    if !genre_data.comps_ebook.is_empty() {
        context_section.push(String::new());
        context_section.push("**Ebook comps:**".to_string());
        for c in &genre_data.comps_ebook { context_section.push(format!("- {}", c)); }
    }
    if !genre_data.comps_print.is_empty() {
        context_section.push(String::new());
        context_section.push("**Print comps:**".to_string());
        for c in &genre_data.comps_print { context_section.push(format!("- {}", c)); }
    }
    context_section.push(String::new());
    report_sections.push(context_section.join("\n"));

    let now = chrono::Utc::now().format("%B %-d, %Y %H:%M UTC").to_string();
    let mut lines = vec![
        "# Find Genres & Categories".to_string(),
        format!("Generated: {}", now),
        "Full pipeline in one pass: genre ranking, KDP categories (Kindle eBook + Paperback, with a live Publisher Rocket check when the 3rd/4th slot is a close call), BISAC classification (ebook + print), and positioning context.".to_string(),
        String::new(), "---".to_string(), String::new(),
    ];
    lines.push(report_sections.join("\n---\n\n"));
    let report = lines.join("\n");

    { let conn = database.0.lock().unwrap(); let _ = db::save_document(&conn, &request.folder, "genres_and_categories", &report); }
    emit(&app, "✓ Genres & Categories report saved to database.");

    GenreResult { success: true, report, error: String::new() }
}

// ── Combined Report Assembly ──────────────────────────────────────────────────

/// Assembles all pipeline output sections into a single structured JSON document.
fn render_combined_report(
    kdp_paste_section: &str,
    genre_ranking_section: &str,
    kdp_categories_section: &str,
    bisac_section: &str,
    kdp_keywords_section: &str,
    discovery_keywords_section: &str,
    positioning_section: &str,
) -> String {
    let json = serde_json::json!({
        "schema": "analysis_v1",
        "sections": {
            "kdp_paste": kdp_paste_section,
            "genre_ranking": genre_ranking_section,
            "kdp_categories": kdp_categories_section,
            "bisac": bisac_section,
            "kdp_keywords": kdp_keywords_section,
            "discovery_keywords": discovery_keywords_section,
            "positioning": positioning_section,
        }
    });
    json.to_string()
}

// ── Unified analyze_story command ─────────────────────────────────────────────

#[tauri::command]
pub async fn analyze_story(app: AppHandle, request: AnalyzeStoryRequest) -> GenreResult {
    let database = app.state::<db::Db>();

    // ── Step 1: Summaries ──────────────────────────────────────────────────
    emit(&app, "Step 1: Chapter summaries...");
    {
        let app_c = app.clone();
        let folder_c = request.folder.clone();
        let provider_c = request.provider.clone();
        let api_key_c = request.api_key.clone();
        let model_c = request.model.clone();
        let force = request.force_resummarize;

        let result: Result<(), String> = tokio::task::spawn_blocking(move || {
            let folder_path = PathBuf::from(&folder_c);
            if !folder_path.exists() { return Err("Folder does not exist.".to_string()); }

            crate::reset_cancel();
            let database = app_c.state::<db::Db>();

            if force {
                emit(&app_c, "  Force re-summarize — deleting existing summaries...");
                let conn = database.0.lock().unwrap();
                let _ = db::delete_chapter_summaries(&conn, &folder_c);
            }

            let chapters = collect_chapters(&folder_path);
            if chapters.is_empty() { return Err("No .md chapter files found.".to_string()); }

            let (done, skipped) = phase1_summaries(&app_c, &database, &chapters, &folder_c, &provider_c, &api_key_c, &model_c);
            emit(&app_c, &format!("  ✓ {} summarized, {} skipped.", done, skipped));
            Ok(())
        }).await.unwrap();

        if let Err(e) = result { return err(&e); }
    }
    // Save chapter summaries as a standalone report
    {
        let conn = database.0.lock().unwrap();
        let summaries = db::load_chapter_summaries(&conn, &request.folder);
        if !summaries.is_empty() {
            let cs_json = serde_json::json!({
                "schema": "chapter_summaries_v1",
                "chapters": summaries.iter().map(|s| serde_json::json!({
                    "file": s.file, "title": s.title, "signals": s.signals, "word_count": s.word_count,
                })).collect::<Vec<_>>(),
                "total_words": summaries.iter().map(|s| s.word_count).sum::<i64>(),
            }).to_string();
            let _ = db::save_document(&conn, &request.folder, "chapter_summaries", &cs_json);
        }
    }
    if crate::is_cancelled() { return err("Cancelled."); }

    // ── Step 2: Genre Analysis ─────────────────────────────────────────────
    emit(&app, "Step 2: Genre analysis...");
    let genre_data = { let conn = database.0.lock().unwrap(); db::load_genre_data(&conn, &request.folder) };
    if genre_data.is_none() {
        let app_c = app.clone();
        let folder_c = request.folder.clone();
        let provider_c = request.provider.clone();
        let api_key_c = request.api_key.clone();
        let model_c = request.model.clone();

        let result: Result<(), String> = tokio::task::spawn_blocking(move || {
            let database = app_c.state::<db::Db>();
            let summaries = { let conn = database.0.lock().unwrap(); db::load_chapter_summaries(&conn, &folder_c) };
            if summaries.is_empty() { return Err("No chapter summaries available.".to_string()); }
            let r = phase2_analyze(&app_c, &database, &folder_c, &summaries, &provider_c, &api_key_c, &model_c);
            if !r.success { return Err(r.error); }
            Ok(())
        }).await.unwrap();

        if let Err(e) = result { return err(&e); }
    } else {
        emit(&app, "  Genre data exists — skipping.");
    }
    if crate::is_cancelled() { return err("Cancelled."); }

    let genre_data = { let conn = database.0.lock().unwrap(); db::load_genre_data(&conn, &request.folder) };
    let genre_data = match genre_data {
        Some(d) => d,
        None    => return err("Could not produce genre data."),
    };

    // ── Step 3: Rank Genres ────────────────────────────────────────────────
    emit(&app, "Step 3: Ranking genres...");
    let ranked: Vec<RankedGenre> = {
        let app_c = app.clone();
        let folder_c = request.folder.clone();
        let provider_c = request.provider.clone();
        let api_key_c = request.api_key.clone();
        let model_c = request.model.clone();
        let industry_ebook = genre_data.industry_ebook.clone();
        let kdp_ebook_joined = genre_data.kdp_ebook.join("; ");
        let genre_signals = genre_data.genre_signals.clone();

        let res: Result<Vec<RankedGenre>, String> = tokio::task::spawn_blocking(move || -> Result<Vec<RankedGenre>, String> {
            let database = app_c.state::<db::Db>();
            let master_list = crate::genre_taxonomy::master_genre_list(&database)
                .map_err(|e| format!("Could not load genre list from database: {}", e))?;
            let ai_ranked = ai_rank_genres(
                &provider_c, &api_key_c, &model_c,
                &format!("{}\n\nKDP paths already identified: {}\n\n{}", industry_ebook, kdp_ebook_joined, genre_signals),
                &master_list,
            )?;
            let mut ranked: Vec<RankedGenre> = ai_ranked.into_iter().map(|r| {
                let kdp_paths = crate::genre_taxonomy::kdp_paths_for_genre(&database, &r.genre, "Kindle").unwrap_or_default();
                RankedGenre { genre: r.genre, confidence: r.confidence, reason: r.reason, kdp_paths }
            }).collect();
            ranked.sort_by(|a, b| b.confidence.cmp(&a.confidence));

            let conn = database.0.lock().unwrap();
            let rows: Vec<(String, u8, String)> = ranked.iter().map(|r| (r.genre.clone(), r.confidence, r.reason.clone())).collect();
            let _ = db::replace_genre_rankings(&conn, &folder_c, &rows);

            // Save genre ranking as a standalone report
            let ranking_json = serde_json::json!({
                "schema": "genre_ranking_v1",
                "genres": ranked.iter().map(|r| serde_json::json!({
                    "genre": r.genre, "confidence": r.confidence, "reason": r.reason,
                })).collect::<Vec<_>>(),
            }).to_string();
            let _ = db::save_document(&conn, &folder_c, "genre_ranking", &ranking_json);

            Ok(ranked)
        }).await.unwrap();

        match res {
            Ok(r) => r,
            Err(e) => return err(&format!("Genre ranking failed: {}", e)),
        }
    };
    for r in &ranked { emit(&app, &format!("  {}% — {}", r.confidence, r.genre)); }
    if crate::is_cancelled() { return err("Cancelled."); }

    // Build genre ranking report section as JSON
    let genre_ranking_section = serde_json::json!({
        "genres": ranked.iter().map(|r| serde_json::json!({
            "genre": r.genre,
            "confidence": r.confidence,
            "reason": r.reason,
        })).collect::<Vec<_>>(),
    }).to_string();

    let genre_terms: Vec<(String, u8)> = if !ranked.is_empty() {
        ranked.iter().filter(|r| r.confidence >= 30).take(6).map(|r| (r.genre.clone(), r.confidence)).collect()
    } else {
        vec![(genre_data.industry_ebook.clone(), 100)]
    };

    // ── Step 4: KDP Categories (both stores) ───────────────────────────────
    emit(&app, "Step 4: Matching KDP categories...");
    let base_description = format!("{}\n\n{}", genre_data.industry_ebook, genre_data.genre_signals);
    let mut kdp_stores_json: Vec<serde_json::Value> = Vec::new();
    let mut kindle_top_categories: Vec<String> = Vec::new();
    let mut print_top_categories: Vec<String> = Vec::new();

    for (store, label, top_cats) in [
        ("Kindle", "Kindle eBook", &mut kindle_top_categories as &mut Vec<String>),
        ("Books", "Paperback", &mut print_top_categories as &mut Vec<String>),
    ] {
        let total_catalog = { let conn = database.0.lock().unwrap(); db::kdp_category_count(&conn, store) };
        if total_catalog < 50 {
            kdp_stores_json.push(serde_json::json!({ "store": label, "error": "Catalog nearly empty — import WinningCat data." }));
            continue;
        }

        let app_c = app.clone();
        let folder_c = request.folder.clone();
        let store_c = store.to_string();
        let desc_c = base_description.clone();
        let terms_c = genre_terms.clone();
        let provider_c = request.provider.clone();
        let api_key_c = request.api_key.clone();
        let model_c = request.model.clone();

        let result = tokio::task::spawn_blocking(move || {
            let database = app_c.state::<db::Db>();
            match_categories_by_store(&app_c, &database, &folder_c, &store_c, &desc_c, &terms_c, &provider_c, &api_key_c, &model_c)
        }).await.unwrap();

        let final_cats = rank_by_discoverability(&app, store, result.qualifying, &request.canopy_api_key).await;

        // Extract top 3 category paths for the KDP paste section
        for q in final_cats.iter().take(3) {
            top_cats.push(q.path.clone());
        }

        kdp_stores_json.push(serde_json::json!({
            "store": label,
            "categories": final_cats.iter().enumerate().map(|(i, q)| serde_json::json!({
                "rank": i + 1,
                "path": q.path,
                "fit_confidence": q.fit_confidence,
                "sales_to_ten": q.sales_to_ten,
                "verified": q.verified,
                "is_bonus": i >= 3,
                "agreeing_genres": q.agreeing_genres,
            })).collect::<Vec<_>>(),
        }));

        if crate::is_cancelled() { return err("Cancelled."); }
    }
    let kdp_categories_section = serde_json::json!({ "stores": kdp_stores_json }).to_string();
    if crate::is_cancelled() { return err("Cancelled."); }

    // ── Step 5: BISAC Classification ───────────────────────────────────────
    emit(&app, "Step 5: BISAC classification...");
    let bisac_section: String = {
        let app_c = app.clone();
        let folder_c = request.folder.clone();
        let provider_c = request.provider.clone();
        let api_key_c = request.api_key.clone();
        let model_c = request.model.clone();
        let industry_ebook = genre_data.industry_ebook.clone();
        let industry_print = genre_data.industry_print.clone();
        let genre_signals = genre_data.genre_signals.clone();
        let same_as_ebook = industry_print.trim().eq_ignore_ascii_case(industry_ebook.trim());

        tokio::task::spawn_blocking(move || {
            let database = app_c.state::<db::Db>();
            let bisac_master = { let conn = database.0.lock().unwrap(); db::master_bisac_list(&conn) };

            let ebook_desc = format!("{}\n\n{}", industry_ebook, genre_signals);
            let ebook_picks = ai_pick_bisac(&provider_c, &api_key_c, &model_c, &ebook_desc, &bisac_master).unwrap_or_default();
            {
                let conn = database.0.lock().unwrap();
                let rows: Vec<(String, String, u8, String)> = ebook_picks.iter().map(|(c, h, cf, r)| (c.clone(), h.clone(), *cf, r.clone())).collect();
                let _ = db::replace_bisac_classifications(&conn, &folder_c, "ebook", &rows);
            }

            let print_picks = if same_as_ebook {
                let conn = database.0.lock().unwrap();
                let rows: Vec<(String, String, u8, String)> = ebook_picks.iter().map(|(c, h, cf, r)| (c.clone(), h.clone(), *cf, r.clone())).collect();
                let _ = db::replace_bisac_classifications(&conn, &folder_c, "print", &rows);
                None
            } else {
                let print_desc = format!("{}\n\n{}", industry_print, genre_signals);
                let picks = ai_pick_bisac(&provider_c, &api_key_c, &model_c, &print_desc, &bisac_master).unwrap_or_default();
                let conn = database.0.lock().unwrap();
                let rows: Vec<(String, String, u8, String)> = picks.iter().map(|(c, h, cf, r)| (c.clone(), h.clone(), *cf, r.clone())).collect();
                let _ = db::replace_bisac_classifications(&conn, &folder_c, "print", &rows);
                Some(picks)
            };

            let bisac_json = serde_json::json!({
                "ebook": ebook_picks.iter().map(|(code, heading, conf, reason)| serde_json::json!({
                    "code": code, "heading": heading, "confidence": conf, "reason": reason,
                })).collect::<Vec<_>>(),
                "print": match &print_picks {
                    None => serde_json::json!("same_as_ebook"),
                    Some(picks) => serde_json::json!(picks.iter().map(|(code, heading, conf, reason)| serde_json::json!({
                        "code": code, "heading": heading, "confidence": conf, "reason": reason,
                    })).collect::<Vec<_>>()),
                },
            });
            bisac_json.to_string()
        }).await.unwrap()
    };
    // Save BISAC as standalone report
    { let conn = database.0.lock().unwrap(); let _ = db::save_document(&conn, &request.folder, "bisac_classification", &bisac_section); }
    if crate::is_cancelled() { return err("Cancelled."); }

    // ── Step 6: Keyword Search (PR) ────────────────────────────────────────
    emit(&app, "Step 6: Publisher Rocket keyword search...");
    let keyword_pool: Vec<KeywordResult> = {
        let top_cats_for_seeds: Vec<String> = kindle_top_categories.iter().take(2).cloned().collect();
        let seeds = derive_keyword_seeds(&genre_data.industry_ebook, &top_cats_for_seeds);
        if seeds.is_empty() {
            emit(&app, "  ⚠ No seeds derived — skipping keyword search.");
            Vec::new()
        } else {
            emit(&app, &format!("  Seeds: {:?}", seeds));
            if request.canopy_api_key.is_empty() {
                emit(&app, "  ⚠ No Canopy API key — skipping keyword search.");
                Vec::new()
            } else {
                run_keyword_searches_canopy(&app, &request.folder, &seeds, &request.canopy_api_key).await
            }
        }
    };
    // Save keyword search as standalone report
    if !keyword_pool.is_empty() {
        let conn = database.0.lock().unwrap();
        let ks_json = serde_json::json!({
            "schema": "keyword_search_v1",
            "keywords": keyword_pool.iter().map(|k| serde_json::json!({
                "keyword": k.keyword, "searches": k.searches, "competition": k.competition, "earnings": k.estimated_earnings,
            })).collect::<Vec<_>>(),
        }).to_string();
        let _ = db::save_document(&conn, &request.folder, "keyword_search", &ks_json);
    }
    if crate::is_cancelled() { return err("Cancelled."); }

    // ── Step 7: KDP Keywords ───────────────────────────────────────────────
    emit(&app, "Step 7: Optimizing KDP keywords...");
    let (kdp_keyword_entries, kdp_keyword_strategy) = {
        let provider_c = request.provider.clone();
        let api_key_c = request.api_key.clone();
        let model_c = request.model.clone();
        let genre_data_c = genre_data.clone();
        let genre_signals_c = genre_data.genre_signals.clone();
        let pool_c = keyword_pool.clone();

        let res: Result<(Vec<db::KdpKeywordEntry>, String), String> = tokio::task::spawn_blocking(move || {
            call_keyword_optimizer_with_pool(&provider_c, &api_key_c, &model_c, &genre_data_c, &genre_signals_c, &pool_c)
        }).await.unwrap();

        match res {
            Ok((entries, strategy)) => {
                let source_note = if keyword_pool.is_empty() {
                    "*(Generated from genre analysis — no PR data available.)*"
                } else {
                    "*(Enhanced with real Publisher Rocket search volume data.)*"
                };
                let conn = database.0.lock().unwrap();
                let _ = db::save_kdp_keywords(&conn, &request.folder, &entries, &strategy, source_note);
                emit(&app, &format!("  ✓ {} KDP keyword strings saved.", entries.len()));
                (entries, strategy)
            }
            Err(e) => {
                emit(&app, &format!("  ⚠ KDP keyword optimization failed: {} — continuing.", e));
                (Vec::new(), String::new())
            }
        }
    };
    if crate::is_cancelled() { return err("Cancelled."); }

    // ── Step 8: Discovery Keywords ─────────────────────────────────────────
    emit(&app, "Step 8: Generating discovery keywords...");
    let discovery_entries: Vec<db::DiscoveryKeywordEntry> = {
        let provider_c = request.provider.clone();
        let api_key_c = request.api_key.clone();
        let model_c = request.model.clone();
        let genre_data_c = genre_data.clone();

        let res: Result<Vec<db::DiscoveryKeywordEntry>, String> = tokio::task::spawn_blocking(move || {
            generate_discovery_keywords(&provider_c, &api_key_c, &model_c, &genre_data_c)
        }).await.unwrap();

        match res {
            Ok(entries) => {
                let conn = database.0.lock().unwrap();
                let _ = db::save_discovery_keywords(&conn, &request.folder, &entries);
                // Save as standalone report
                let dk_json = serde_json::json!({
                    "schema": "discovery_keywords_v1",
                    "keywords": entries.iter().map(|e| serde_json::json!({ "phrase": e.phrase, "rationale": e.rationale })).collect::<Vec<_>>(),
                }).to_string();
                let _ = db::save_document(&conn, &request.folder, "discovery_keywords", &dk_json);
                emit(&app, &format!("  ✓ {} discovery keywords saved.", entries.len()));
                entries
            }
            Err(e) => {
                emit(&app, &format!("  ⚠ Discovery keywords failed: {} — continuing.", e));
                Vec::new()
            }
        }
    };
    if crate::is_cancelled() { return err("Cancelled."); }

    // ── Step 9: Assemble Combined Report ───────────────────────────────────
    emit(&app, "Step 9: Assembling combined report...");

    // KDP paste section
    let kdp_paste = render_kdp_paste_section(&kindle_top_categories, &print_top_categories, &kdp_keyword_entries);

    // KDP keywords section
    let source_note = if keyword_pool.is_empty() {
        "*(Generated from genre analysis — no PR data available.)*"
    } else {
        "*(Enhanced with real Publisher Rocket search volume data.)*"
    };
    let kdp_keywords_section = render_kdp_keywords(&kdp_keyword_entries, &kdp_keyword_strategy, source_note);

    // Discovery keywords section
    let discovery_keywords_section = serde_json::json!({
        "keywords": discovery_entries.iter().map(|e| serde_json::json!({
            "phrase": e.phrase,
            "rationale": e.rationale,
        })).collect::<Vec<_>>(),
    }).to_string();

    // Positioning context section
    let positioning_section = serde_json::json!({
        "reader_demographic": genre_data.reader_demographic,
        "bookstore_shelving": genre_data.bookstore_shelving,
        "comps_ebook": genre_data.comps_ebook,
        "comps_print": genre_data.comps_print,
    }).to_string();

    let report = render_combined_report(
        &kdp_paste,
        &genre_ranking_section,
        &kdp_categories_section,
        &bisac_section,
        &kdp_keywords_section,
        &discovery_keywords_section,
        &positioning_section,
    );

    { let conn = database.0.lock().unwrap(); let _ = db::save_document(&conn, &request.folder, "analysis", &report); }
    emit(&app, "✓ Full analysis report saved.");

    GenreResult { success: true, report, error: String::new() }
}

// ── Genre Ranking (independent-scored, persisted) ────────────────────────────
//
// Scores the manuscript against the master genre list in the database.
// Scores are independent, not normalized to sum to 100 — a cross-genre book
// can legitimately score high on several genres at once.

#[derive(Deserialize)]
struct AiGenreRank {
    genre:      String,
    confidence: u8,
    reason:     String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RankedGenre {
    pub genre:      String,
    pub confidence: u8,
    pub reason:     String,
    pub kdp_paths:  Vec<String>,
}

#[tauri::command]
pub async fn rank_genres_for_story(app: AppHandle, request: FolderRequest) -> GenreResult {
    tokio::task::spawn_blocking(move || {
        let database = app.state::<db::Db>();

        let genre_data = { let conn = database.0.lock().unwrap(); db::load_genre_data(&conn, &request.folder) };
        let genre_data = match genre_data {
            Some(d) => d,
            None    => return err("No genre data found. Run Analyze first."),
        };

        let description = format!(
            "{}\n\nKDP paths already identified from manuscript analysis: {}\n\n{}",
            genre_data.industry_ebook, genre_data.kdp_ebook.join("; "), genre_data.genre_signals
        );

        emit(&app, "Ranking manuscript against master genre list...");
        let master_list = match crate::genre_taxonomy::master_genre_list(&database) {
            Ok(l) => l,
            Err(e) => return err(&format!("Could not load genre list from database: {}", e)),
        };
        emit(&app, &format!("  Scoring against {} known genres.", master_list.len()));

        match ai_rank_genres(&request.provider, &request.api_key, &request.model, &description, &master_list) {
            Err(e) => err(&e),
            Ok(ai_ranked) => {
                let mut ranked: Vec<RankedGenre> = ai_ranked.into_iter().map(|r| {
                    let kdp_paths = crate::genre_taxonomy::kdp_paths_for_genre(&database, &r.genre, "Kindle")
                        .unwrap_or_default();
                    RankedGenre { genre: r.genre, confidence: r.confidence, reason: r.reason, kdp_paths }
                }).collect();
                ranked.sort_by(|a, b| b.confidence.cmp(&a.confidence));

                for r in &ranked {
                    emit(&app, &format!("  {}% — {}{}", r.confidence, r.genre,
                        if r.kdp_paths.is_empty() { " (no mapped KDP path yet)".to_string() } else { String::new() }));
                }

                let now_disp = chrono::Utc::now().format("%B %-d, %Y %H:%M UTC").to_string();
                let mut lines = vec![
                    "# Genre Ranking".to_string(),
                    format!("Generated: {}", now_disp),
                    String::new(),
                    "Each genre is scored independently against the manuscript — percentages do NOT sum to 100. A cross-genre book can score high on several genres at once; a lower score means a weaker but still real fit.".to_string(),
                    String::new(),
                ];
                for r in &ranked {
                    lines.push(format!("## {} — {}%", r.genre, r.confidence));
                    lines.push(String::new());
                    lines.push(r.reason.clone());
                    lines.push(String::new());
                    if !r.kdp_paths.is_empty() {
                        lines.push("**Known KDP category path(s):**".to_string());
                        for p in &r.kdp_paths { lines.push(format!("- `{}`", p)); }
                    } else {
                        lines.push("*No mapped KDP path yet for this genre. Run Category Finder to discover one — it will be saved to the database automatically.*".to_string());
                    }
                    lines.push(String::new());
                    lines.push("---".to_string());
                    lines.push(String::new());
                }
                let report = lines.join("\n");

                let conn = database.0.lock().unwrap();
                let rows: Vec<(String, u8, String)> = ranked.iter()
                    .map(|r| (r.genre.clone(), r.confidence, r.reason.clone()))
                    .collect();
                if let Err(e) = db::replace_genre_rankings(&conn, &request.folder, &rows) {
                    emit(&app, &format!("  ⚠ Could not save ranking to database: {}", e));
                }
                let _ = db::save_document(&conn, &request.folder, "genre_ranking", &report);
                emit(&app, &format!("✓ Ranking saved to database — {} genre(s) ranked.", ranked.len()));

                GenreResult { success: true, report, error: String::new() }
            }
        }
    }).await.unwrap()
}

fn ai_rank_genres(provider: &str, api_key: &str, model: &str, description: &str, master_list: &[db::GenreRow])
    -> Result<Vec<AiGenreRank>, String>
{
    let listing = master_list.iter()
        .map(|g| format!("- {}: {}", g.name, g.description))
        .collect::<Vec<_>>()
        .join("\n");

    let system = format!(
        r#"You are a publishing genre classification expert. Score this book INDEPENDENTLY against EACH genre in the list below. Do not normalize or force scores to sum to 100 — a genuinely cross-genre book can score high (even 80%+) on more than one genre at once, and that is correct, not an error.

Return ONLY a JSON array, no markdown, no preamble.
Include only genres scoring above 15.
Each item: {{ "genre": "<exact name from list>", "confidence": <0-100>, "reason": "<one sentence>" }}
Sort descending by confidence.

Genre list:
{}"#,
        listing
    );

    let raw = call_llm(provider, api_key, model, &system, description, 1200)?;
    let clean = raw.trim()
        .trim_start_matches("```json").trim_start_matches("```")
        .trim_end_matches("```").trim();

    serde_json::from_str::<Vec<AiGenreRank>>(clean)
        .map_err(|e| format!("Parse error (genre ranking): {} | got: {}", e, &clean[..clean.len().min(300)]))
}

// ── Verify Mapped Categories ──────────────────────────────────────────────────
//
// Takes the genres already ranked for this story, collects every known KDP
// path, and runs them through the Category Analyzer (exact-path verification
// against live Publisher Rocket data) — no AI call needed, paths are known.

#[derive(Deserialize)]
pub struct VerifyMappedRequest {
    pub folder: String,
    pub store:  String,
}

#[tauri::command]
pub async fn verify_mapped_categories(app: AppHandle, db: tauri::State<'_, db::Db>, request: VerifyMappedRequest) -> Result<GenreResult, ()> {
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
        return Ok(err("None of this story's ranked genres have a mapped KDP path yet. Run Category Finder to discover paths — they'll be saved to the database automatically."));
    }

    emit(&app, &format!("Verifying {} mapped KDP path(s) in Publisher Rocket...", paths.len()));

    let result = crate::commands::analyze_categories(
        app.clone(),
        crate::commands::CategoryRequest {
            paths,
            store:  request.store.clone(),
            filter: "Selectable Excluding Ghosts".to_string(),
        },
    ).await;

    if !result.success {
        return Ok(err(&result.error));
    }

    {
        let conn = db.0.lock().unwrap();
        let _ = db::save_document(&conn, &request.folder, "mapped_categories", &result.markdown);
        for g in &rankings {
            for p in &g.kdp_paths {
                let _ = db::upsert_kdp_path(&conn, &g.genre, p, &request.store, "category_analyzer", true);
            }
        }
    }

    emit(&app, "✓ Verified paths saved to database.");

    Ok(GenreResult { success: true, report: result.markdown, error: String::new() })
}

// ── Keyword Optimizer ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct KeywordRequest {
    pub folder:   String,
    pub api_key:  String,
    pub model:    String,
    pub provider: String,
}

#[tauri::command]
pub async fn generate_pr_keywords(app: AppHandle, request: KeywordRequest) -> GenreResult {
    tokio::task::spawn_blocking(move || {
        let database = app.state::<db::Db>();
        let genre_data = { let conn = database.0.lock().unwrap(); db::load_genre_data(&conn, &request.folder) };
        let genre_data = match genre_data {
            Some(d) => d,
            None    => return err("No genre data found. Run Analyze first."),
        };

        emit(&app, "Generating PR Competition Analyzer search terms...");
        emit(&app, &format!("  Genre: {}", genre_data.industry_ebook));

        let system = r#"You are a Publisher Rocket expert. Generate short search phrases for the Competition Analyzer tool.

Publisher Rocket Competition Analyzer works like Amazon search — it needs SHORT, SPECIFIC phrases that real readers type.

Rules:
- 2-4 words maximum per phrase
- Plain English, no special characters
- Think like a reader browsing Amazon, not a marketer
- Phrases should find competing books in the same genre niche
- Include: genre combinations, setting descriptors, theme words, reader mood phrases

Return ONLY a JSON array of strings. No markdown, no preamble. Example:
["christian historical fiction", "first century rome", "biblical mystery", "faith romance clean"]"#;

        let user = format!(
            "Book genre: {}\nKDP categories: {}\nGenre signals:\n{}",
            genre_data.industry_ebook,
            genre_data.kdp_ebook.iter()
                .map(|p| p.split('>').last().unwrap_or(p).trim().to_string())
                .collect::<Vec<_>>().join(", "),
            &genre_data.genre_signals[..genre_data.genre_signals.len().min(500)]
        );

        match call_llm(&request.provider, &request.api_key, &request.model, system, &user, 300) {
            Err(e) => err(&format!("AI error: {}", e)),
            Ok(raw) => {
                let clean = raw.trim()
                    .trim_start_matches("```json").trim_start_matches("```")
                    .trim_end_matches("```").trim();

                match serde_json::from_str::<Vec<String>>(clean) {
                    Err(e) => err(&format!("Parse error: {} | got: {}", e, &clean[..clean.len().min(200)])),
                    Ok(keywords) => {
                        emit(&app, &format!("  ✓ {} PR search terms generated:", keywords.len()));
                        for kw in &keywords { emit(&app, &format!("    • {}", kw)); }

                        let rendered = render_pr_keywords(&keywords);
                        let conn = database.0.lock().unwrap();
                        let _ = db::save_pr_keywords(&conn, &request.folder, &keywords);
                        let _ = db::save_document(&conn, &request.folder, "pr_keywords", &rendered);

                        GenreResult { success: true, report: rendered, error: String::new() }
                    }
                }
            }
        }
    }).await.unwrap()
}

#[tauri::command]
pub async fn optimize_keywords(app: AppHandle, request: KeywordRequest) -> GenreResult {
    tokio::task::spawn_blocking(move || {
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

        match call_keyword_optimizer(&request.provider, &request.api_key, &request.model, &genre_data, &genre_data.genre_signals) {
            Err(e) => err(&format!("AI error: {}", e)),
            Ok((entries, strategy)) => {
                let rendered = render_kdp_keywords(&entries, &strategy, source_note);
                let conn = database.0.lock().unwrap();
                let _ = db::save_kdp_keywords(&conn, &request.folder, &entries, &strategy, source_note);
                let _ = db::save_document(&conn, &request.folder, "kdp_keywords", &rendered);
                emit(&app, "✓ KDP keywords saved to database.");
                GenreResult { success: true, report: rendered, error: String::new() }
            }
        }
    }).await.unwrap()
}

fn call_keyword_optimizer(provider: &str, api_key: &str, model: &str, genre_data: &db::GenreDataRow, keywords_text: &str)
    -> Result<(Vec<db::KdpKeywordEntry>, String), String>
{
    let system = r#"You are an Amazon KDP keyword strategist helping an indie author maximize book discoverability.

Produce exactly 7 KDP keyword strings ready to paste into the KDP keyword fields.

Rules:
- Each string must be 50 characters or fewer (hard limit — count carefully)
- Natural search phrases a reader would actually type on Amazon
- Multi-word phrases only — Amazon already indexes your title and categories
- Do NOT repeat words already in the book's categories
- Vary the strings: setting, theme, reader mood, comp authors, tropes
- No punctuation except spaces and hyphens
- All lowercase

Return ONLY a JSON object:
{
  "keywords": [
    { "string": "the phrase", "chars": 10, "rationale": "one sentence why" },
    ... (exactly 7 items)
  ],
  "strategy": "One paragraph on the overall keyword strategy."
}"#;

    let user = format!(
        "Genre (ebook): {}\nGenre (print): {}\nKDP ebook categories: {}\nKDP print categories: {}\n\nKeyword material:\n\n{}",
        genre_data.industry_ebook, genre_data.industry_print,
        genre_data.kdp_ebook.join(", "), genre_data.kdp_print.join(", "),
        keywords_text
    );

    let raw = call_llm(provider, api_key, model, system, &user, 1000)?;
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

// ── Enhanced KDP keyword optimizer (with real Publisher Rocket data) ──────────

fn format_keyword_pool_table(pool: &[KeywordResult]) -> String {
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

fn call_keyword_optimizer_with_pool(
    provider: &str,
    api_key: &str,
    model: &str,
    genre_data: &db::GenreDataRow,
    keywords_text: &str,
    keyword_pool: &[KeywordResult],
) -> Result<(Vec<db::KdpKeywordEntry>, String), String> {
    if keyword_pool.is_empty() {
        return call_keyword_optimizer(provider, api_key, model, genre_data, keywords_text);
    }

    let pool_table = format_keyword_pool_table(keyword_pool);

    let system = r#"You are an Amazon KDP keyword strategist helping an indie author maximize book discoverability.

Produce exactly 7 KDP keyword strings ready to paste into the KDP keyword fields.

Rules:
- Each string must be 50 characters or fewer (hard limit — count carefully)
- Natural search phrases a reader would actually type on Amazon
- Multi-word phrases only — Amazon already indexes your title and categories
- Do NOT repeat words already in the book's categories
- Vary the strings: setting, theme, reader mood, comp authors, tropes
- No punctuation except spaces and hyphens
- All lowercase
- PREFER keywords from the real Publisher Rocket data provided — these have measured Amazon search volume
- When a keyword comes from real data, include its search volume in the rationale: "real: X searches/mo"
- When a keyword is AI-derived (not from the real data table), note it: "AI-derived"

Return ONLY a JSON object:
{
  "keywords": [
    { "string": "the phrase", "chars": 10, "rationale": "real: 1,200 searches/mo — one sentence why" },
    ... (exactly 7 items)
  ],
  "strategy": "One paragraph on the overall keyword strategy."
}"#;

    let user = format!(
        "Genre (ebook): {}\nGenre (print): {}\n\
         KDP ebook categories: {}\nKDP print categories: {}\n\n\
         ## Real Publisher Rocket Keyword Data\n\
         (Prefer these — they have measured Amazon search volume)\n\n\
         {}\n\n\
         ## Additional keyword material\n\n{}",
        genre_data.industry_ebook, genre_data.industry_print,
        genre_data.kdp_ebook.join(", "), genre_data.kdp_print.join(", "),
        pool_table, keywords_text
    );

    let raw = call_llm(provider, api_key, model, system, &user, 1200)?;
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

// ── Discovery keyword generator (non-Amazon platforms) ────────────────────────

/// Generate 10 discovery keyword phrases for non-Amazon platforms (Apple Books,
/// Kobo, Google Play, B&N, BookBub, Goodreads, general web). Each phrase is
/// AI-reasoned and labeled as such — these are not backed by any measured data.
pub fn generate_discovery_keywords(
    provider: &str,
    api_key: &str,
    model: &str,
    genre_data: &db::GenreDataRow,
) -> Result<Vec<db::DiscoveryKeywordEntry>, String> {
    let system = r#"You are a book marketing strategist for non-Amazon platforms.

Produce exactly 10 discovery keyword phrases optimized for these platforms:
Apple Books, Kobo, Google Play, Barnes & Noble, BookBub, Goodreads, and general web search (SEO).

Rules:
- Natural search phrases a reader would type on these platforms
- Multi-word phrases (2-5 words)
- Cover different discovery angles: genre, theme, mood, comp authors, setting, tropes
- These are NOT measured by any tool — label each rationale with the prefix "AI-reasoned:"

Return ONLY a JSON object:
{
  "keywords": [
    { "phrase": "the phrase", "rationale": "AI-reasoned: one sentence why this phrase works for non-Amazon discovery" },
    ... (exactly 10 items)
  ]
}"#;

    let user = format!(
        "Genre (ebook): {}\nGenre (print): {}\nReader demographic: {}\nBookstore shelving: {}\nGenre signals: {}",
        genre_data.industry_ebook,
        genre_data.industry_print,
        genre_data.reader_demographic,
        genre_data.bookstore_shelving,
        genre_data.genre_signals
    );

    let raw = call_llm(provider, api_key, model, system, &user, 1200)?;
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

// ── Rendering (structured DB data → markdown, always fresh, never cached-stale) ─

fn render_genre_analysis_md(g: &db::GenreDataRow) -> String {
    let json = serde_json::json!({
        "schema": "genre_analysis_v1",
        "industry_ebook": g.industry_ebook,
        "industry_print": g.industry_print,
        "comps_ebook": g.comps_ebook,
        "comps_print": g.comps_print,
        "reader_demographic": g.reader_demographic,
        "bookstore_shelving": g.bookstore_shelving,
        "kdp_ebook": g.kdp_ebook,
        "kdp_print": g.kdp_print,
        "genre_signals": g.genre_signals,
        "marketing_notes": g.marketing_notes,
    });
    json.to_string()
}

fn render_full_report(g: &db::GenreDataRow, competition_done: bool) -> String {
    let json = serde_json::json!({
        "schema": "full_report_v1",
        "genre_analysis": serde_json::from_str::<serde_json::Value>(&render_genre_analysis_md(g)).unwrap_or_default(),
        "competition_done": competition_done,
    });
    json.to_string()
}

fn render_kdp_keywords(entries: &[db::KdpKeywordEntry], strategy: &str, source_note: &str) -> String {
    // Store as structured JSON — the frontend renders to HTML
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

fn render_pr_keywords(keywords: &[String]) -> String {
    let json = serde_json::json!({
        "schema": "pr_keywords_v1",
        "keywords": keywords,
    });
    json.to_string()
}

// ── Phase implementations ─────────────────────────────────────────────────────

fn phase1_summaries(app: &AppHandle, database: &db::Db, chapters: &[PathBuf], story_folder: &str, provider: &str, api_key: &str, model: &str)
    -> (usize, usize)
{
    let mut done = 0usize;
    let mut skipped = 0usize;

    for (i, chapter_path) in chapters.iter().enumerate() {
        let fname = chapter_path.file_name().unwrap_or_default().to_string_lossy().to_string();

        let already_done = { let conn = database.0.lock().unwrap(); db::chapter_summary_exists(&conn, story_folder, &fname) };
        if already_done {
            emit(app, &format!("  [{}/{}] SKIP: {}", i + 1, chapters.len(), fname));
            skipped += 1;
            continue;
        }

        emit(app, &format!("  [{}/{}] Summarizing: {}", i + 1, chapters.len(), fname));

        let content = match fs::read_to_string(chapter_path) {
            Ok(c) if !c.trim().is_empty() => c,
            Ok(_)  => { emit(app, "    ⚠ Empty — skipping."); continue; }
            Err(e) => { emit(app, &format!("    ⚠ Read error: {}", e)); continue; }
        };

        let word_count = content.split_whitespace().count();
        emit(app, &format!("    {} words", word_count));

        match summarize_chapter(provider, api_key, model, &fname, &truncate_words(&content, 8000)) {
            Ok(signals) => {
                let title = extract_title(&content).unwrap_or_else(|| fname.clone());
                let conn = database.0.lock().unwrap();
                let _ = db::save_chapter_summary(&conn, story_folder, &fname, &title, &signals, word_count as i64);
                emit(app, &format!("    ✓ Done ({} signal chars)", signals.len()));
                done += 1;
            }
            Err(e) => emit(app, &format!("    ⚠ AI error: {}", e)),
        }

        if crate::is_cancelled() { emit(app, "⚠ Cancelled."); break; }
    }

    emit(app, &format!("Phase 1 complete — {} new, {} skipped.", done, skipped));
    (done, skipped)
}

fn phase2_analyze(app: &AppHandle, database: &db::Db, story_folder: &str, summaries: &[db::ChapterSummaryRow], provider: &str, api_key: &str, model: &str)
    -> GenreResult
{
    let combined = build_combined_context(summaries);

    emit(app, &format!(
        "  Sending {} summaries ({} chars) to {}...",
        summaries.len(), combined.len(), model
    ));

    match call_ai_genre_analysis(provider, api_key, model, &combined) {
        Err(e) => err(&format!("Phase 2 AI error: {}", e)),
        Ok(g) => {
            let conn = database.0.lock().unwrap();
            let _ = db::save_genre_data(
                &conn, story_folder,
                &g.industry_ebook, &g.industry_print, &g.genre_signals,
                &g.reader_demographic, &g.bookstore_shelving,
                &g.kdp_ebook, &g.kdp_print, &g.comps_ebook, &g.comps_print, &g.marketing_notes,
            );
            emit(app, "  ✓ Genre data saved to database.");
            let rendered = render_genre_analysis_md(&g);
            let _ = db::save_document(&conn, story_folder, "genre_analysis", &rendered);
            GenreResult { success: true, report: rendered, error: String::new() }
        }
    }
}

// ── AI calls ──────────────────────────────────────────────────────────────────

fn summarize_chapter(provider: &str, api_key: &str, model: &str, filename: &str, content: &str)
    -> Result<String, String>
{
    let system = r#"You are a literary analyst specializing in book genre classification for publishing.

Extract ONLY the genre signals from this chapter — not a plot summary.

Cover:
- Setting: time period, location, world type (historical, contemporary, fantasy, etc.)
- Tone: dark/cozy/literary/commercial/inspirational/gritty/romantic
- Themes: faith, redemption, justice, love, survival, identity, etc.
- Conflict type: internal/external, romantic, mystery, adventure, spiritual
- Romantic elements: present/absent, heat level (clean/sweet/sensual/explicit)
- Faith/spiritual elements: Christian, general spiritual, secular
- Supernatural: none/mild/heavy
- Pacing: fast/slow, action/character/dialogue driven
- Narrative voice: POV, distance, register
- Recognizable tropes or genre conventions

Write 2-3 dense paragraphs. Be specific."#;

    call_llm(provider, api_key, model, system,
        &format!("Chapter: {}\n\n---\n\n{}", filename, content), 600)
}

fn call_ai_genre_analysis(provider: &str, api_key: &str, model: &str, combined: &str)
    -> Result<db::GenreDataRow, String>
{
    let system = r#"You are a senior publishing consultant specializing in Amazon KDP and the broader ebook/print marketplace.

Analyze the provided chapter genre-signal summaries and return a JSON object with this EXACT structure:

{
  "industry_ebook": "Primary genre / subgenre for ebook market",
  "industry_print": "Primary genre / subgenre for print market",
  "kdp_ebook": ["Full > Category > Path", "Second > Full > Path"],
  "kdp_print": ["Full > Category > Path", "Second > Full > Path"],
  "genre_signals": "One paragraph summary of dominant genre signals.",
  "comps_ebook": ["Title by Author (Year)", "Title by Author (Year)"],
  "comps_print": ["Title by Author (Year)", "Title by Author (Year)"],
  "reader_demographic": "Description of the target reader",
  "bookstore_shelving": "Where this would be shelved in a physical bookstore",
  "marketing_notes": ["Note 1", "Note 2", "Note 3"]
}

Rules:
- KDP paths must be real, full paths from the Kindle Store category tree
- Include exactly 2 category paths per format (ebook and print)
- Return ONLY the JSON object, no markdown fences, no preamble"#;

    let raw = call_llm(provider, api_key, model, system,
        &format!("Genre signals from all chapters:\n\n{}", combined), 1500)?;

    let clean = extract_json_object(&raw)
        .ok_or_else(|| format!("No JSON object found: {}", &raw[..raw.len().min(200)]))?;

    let v: serde_json::Value = serde_json::from_str(&clean)
        .map_err(|e| format!("JSON parse: {} | got: {}", e, &clean[..clean.len().min(400)]))?;

    let str_field = |key: &str| v[key].as_str().unwrap_or("").to_string();
    let str_arr   = |key: &str| -> Vec<String> {
        v[key].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).map(String::from).collect())
              .unwrap_or_default()
    };

    Ok(db::GenreDataRow {
        generated_at:       chrono::Utc::now().to_rfc3339(),
        industry_ebook:     str_field("industry_ebook"),
        industry_print:     str_field("industry_print"),
        genre_signals:      str_field("genre_signals"),
        reader_demographic: str_field("reader_demographic"),
        bookstore_shelving: str_field("bookstore_shelving"),
        kdp_ebook:          strip_kdp_paths(str_arr("kdp_ebook")),
        kdp_print:          strip_kdp_paths(str_arr("kdp_print")),
        comps_ebook:        str_arr("comps_ebook"),
        comps_print:        str_arr("comps_print"),
        marketing_notes:    str_arr("marketing_notes"),
    })
}

/// Extract the first complete JSON object from a string.
/// Handles cases where the AI returns extra text before or after the JSON.
fn extract_json_object(text: &str) -> Option<String> {
    let stripped = text.trim()
        .trim_start_matches("```json").trim_start_matches("```")
        .trim_end_matches("```").trim();
    if serde_json::from_str::<serde_json::Value>(stripped).is_ok() {
        return Some(stripped.to_string());
    }
    let start = text.find('{')?;
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    for (i, &b) in bytes[start..].iter().enumerate() {
        if escape { escape = false; continue; }
        match b {
            b'\\' if in_string => escape = true,
            b'"'  => in_string = !in_string,
            b'{'  if !in_string => depth += 1,
            b'}'  if !in_string => {
                depth -= 1;
                if depth == 0 {
                    let candidate = &text[start..start+i+1];
                    if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
                        return Some(candidate.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Strip store-level prefixes from a KDP category path.
/// "Kindle Store > Kindle eBooks > Romance > Contemporary" → "Romance > Contemporary"
/// "Books > Literature & Fiction > Women's Fiction" → "Literature & Fiction > Women's Fiction"
fn strip_kdp_prefix(path: &str) -> String {
    let store_prefixes = [
        "kindle store > kindle ebooks > ",
        "kindle store > kindle books > ",
        "kindle store > ",
        "kindle ebooks > ",
        "kindle books > ",
        "books > ",
        "audible books & originals > ",
        "audible > ",
    ];
    let lower = path.to_lowercase();
    for prefix in &store_prefixes {
        if lower.starts_with(prefix) {
            return path[prefix.len()..].to_string();
        }
    }
    path.to_string()
}

fn strip_kdp_paths(paths: Vec<String>) -> Vec<String> {
    paths.into_iter().map(|p| strip_kdp_prefix(&p)).collect()
}

// ── File helpers (manuscript source files only — these stay on disk) ──────────

fn collect_chapters(folder: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_md_recursive(folder, &mut files);
    files.sort_by(|a, b| {
        natural_sort_key(a.to_string_lossy().as_ref())
            .cmp(&natural_sort_key(b.to_string_lossy().as_ref()))
    });
    files
}

fn collect_md_recursive(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if name.starts_with('.') || name == "_analysis" { continue; }
        if path.is_dir() { collect_md_recursive(&path, out); }
        else if path.extension().map(|e| e == "md").unwrap_or(false) { out.push(path); }
    }
}

fn natural_sort_key(s: &str) -> Vec<u64> {
    let mut key = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() { cur.push(c); }
        else {
            if !cur.is_empty() { key.push(cur.parse::<u64>().unwrap_or(0)); cur.clear(); }
            key.push(c as u64);
        }
    }
    if !cur.is_empty() { key.push(cur.parse::<u64>().unwrap_or(0)); }
    key
}

fn extract_title(content: &str) -> Option<String> {
    content.lines().take(10)
        .find(|l| l.trim().starts_with("# "))
        .map(|l| l.trim().trim_start_matches("# ").trim().to_string())
}

fn truncate_words(text: &str, max: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() <= max { return text.to_string(); }
    words[..max].join(" ") + "\n\n[Truncated]"
}

fn build_combined_context(summaries: &[db::ChapterSummaryRow]) -> String {
    summaries.iter().enumerate().map(|(i, s)| {
        format!("--- Chapter {} ({}, ~{} words) ---\n{}\n\n", i + 1, s.title, s.word_count, s.signals)
    }).collect()
}

fn emit(app: &AppHandle, msg: &str) { let _ = app.emit("genre:log", msg); }

fn err(msg: &str) -> GenreResult {
    GenreResult { success: false, report: String::new(), error: msg.to_string() }
}

// ── Pure helper functions for keyword seed derivation and aggregation ─────────

/// Derive 2-3 seed terms from existing analysis data.
/// Returns a deduplicated Vec of seed strings (case-insensitive dedup).
pub fn derive_keyword_seeds(
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

/// Merge keyword results from multiple seed searches into a single
/// deduplicated pool. When the same keyword appears from multiple seeds,
/// keep the entry with the higher searches value.
pub fn aggregate_keyword_results(
    results_per_seed: Vec<(&str, Vec<KeywordResult>)>,
) -> Vec<KeywordResult> {
    use std::collections::HashMap;
    let mut pool: HashMap<String, KeywordResult> = HashMap::new();

    for (_seed, results) in results_per_seed {
        for r in results {
            let key = r.keyword.to_lowercase();
            let insert = match pool.get(&key) {
                None => true,
                Some(existing) => {
                    parse_searches(&r.searches) > parse_searches(&existing.searches)
                }
            };
            if insert {
                pool.insert(key, r);
            }
        }
    }

    let mut out: Vec<KeywordResult> = pool.into_values().collect();
    out.sort_by(|a, b| {
        parse_searches(&b.searches).cmp(&parse_searches(&a.searches))
    });
    out
}

/// Strip commas from a search volume string and parse to u64.
fn parse_searches(s: &str) -> u64 {
    s.replace(',', "").parse::<u64>().unwrap_or(0)
}

// ── Keyword search aggregation wrapper ────────────────────────────────────────

/// Run Publisher Rocket keyword searches for each seed, aggregate results,
/// and persist to the database. Gracefully handles individual seed failures
/// and cancellation between seeds. Never panics.
pub async fn run_keyword_searches(
    app: &AppHandle,
    folder: &str,
    seeds: &[String],
) -> Vec<KeywordResult> {
    let mut results_per_seed: Vec<(&str, Vec<KeywordResult>)> = Vec::new();

    for seed in seeds {
        // Cancellation check between seeds
        if crate::is_cancelled() {
            let _ = app.emit("cdp:log", "Keyword search cancelled.");
            break;
        }

        let _ = app.emit("cdp:log", &format!("Searching keyword: \"{}\"...", seed));

        let app_clone = app.clone();
        let seed_clone = seed.clone();

        let result = tokio::task::spawn_blocking(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::keyword_search::search_keyword(&app_clone, &seed_clone)
            }))
        })
        .await;

        match result {
            Ok(Ok(Ok(keyword_results))) => {
                let _ = app.emit(
                    "cdp:log",
                    &format!("✓ \"{}\" returned {} keyword(s).", seed, keyword_results.len()),
                );
                // Persist this seed's results to the database
                let database = app.state::<db::Db>();
                let conn = database.0.lock().unwrap();
                let rows: Vec<(String, String, String, String)> = keyword_results
                    .iter()
                    .map(|r| {
                        (
                            r.keyword.clone(),
                            r.searches.clone(),
                            r.competition.clone(),
                            r.estimated_earnings.clone(),
                        )
                    })
                    .collect();
                let _ = db::replace_keyword_search_results(&conn, folder, seed, &rows);
                results_per_seed.push((seed.as_str(), keyword_results));
            }
            Ok(Ok(Err(e))) => {
                let _ = app.emit(
                    "cdp:log",
                    &format!("⚠ Keyword search for \"{}\" failed: {}", seed, e),
                );
            }
            Ok(Err(_panic)) => {
                let _ = app.emit(
                    "cdp:log",
                    &format!("⚠ Keyword search for \"{}\" panicked — skipping.", seed),
                );
            }
            Err(_join_err) => {
                let _ = app.emit(
                    "cdp:log",
                    &format!("⚠ Keyword search task for \"{}\" was cancelled — skipping.", seed),
                );
            }
        }
    }

    if results_per_seed.is_empty() {
        let _ = app.emit("cdp:log", "⚠ All keyword searches failed — returning empty pool.");
        return Vec::new();
    }

    let _ = app.emit(
        "cdp:log",
        &format!(
            "Aggregating results from {} successful seed(s)...",
            results_per_seed.len()
        ),
    );
    let aggregated = aggregate_keyword_results(results_per_seed);
    let _ = app.emit(
        "cdp:log",
        &format!("✓ Keyword pool: {} unique keyword(s) after dedup.", aggregated.len()),
    );
    aggregated
}

/// Canopy-based keyword search — replaces PR keyword search in the pipeline.
/// For each seed: gets autocomplete suggestions, searches top results, estimates volume.
async fn run_keyword_searches_canopy(
    app: &AppHandle,
    folder: &str,
    seeds: &[String],
    canopy_api_key: &str,
) -> Vec<KeywordResult> {
    let mut all_results: Vec<KeywordResult> = Vec::new();

    for seed in seeds {
        if crate::is_cancelled() { break; }

        let app_c = app.clone();
        let seed_c = seed.clone();
        let key_c = canopy_api_key.to_string();
        let folder_c = folder.to_string();

        let result = tokio::task::spawn_blocking(move || {
            let client = crate::canopy::CanopyClient::new(&key_c)?;
            let _ = app_c.emit("cdp:log", &format!("Keyword search (Canopy): \"{}\"", seed_c));

            // Get suggestions
            let suggestions = client.autocomplete(&seed_c, "US", Some("digital-text"))
                .unwrap_or_else(|_| vec![seed_c.clone()]);
            let mut keywords: Vec<String> = vec![seed_c.clone()];
            for s in suggestions.into_iter().take(10) {
                if !keywords.contains(&s) { keywords.push(s); }
            }

            let mut results: Vec<KeywordResult> = Vec::new();
            for kw in &keywords {
                let search = match client.search(kw, "US", Some("digital-text"), 1) {
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
                    if let Ok(s) = client.get_sales(&sr.asin, "US") {
                        if let Some(d) = s.estimated_daily_sales { daily_sales.push(d); }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(80));
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
            let database = app_c.state::<crate::db::Db>();
            let conn = database.0.lock().unwrap();
            let rows: Vec<(String, String, String, String)> = results.iter()
                .map(|r| (r.keyword.clone(), r.searches.clone(), r.competition.clone(), r.estimated_earnings.clone()))
                .collect();
            let _ = crate::db::replace_keyword_search_results(&conn, &folder_c, &seed_c, &rows);

            Ok::<Vec<KeywordResult>, String>(results)
        }).await.unwrap();

        match result {
            Ok(kws) => {
                let _ = app.emit("cdp:log", &format!("✓ \"{}\" → {} keyword(s).", seed, kws.len()));
                all_results.extend(kws);
            }
            Err(e) => { let _ = app.emit("cdp:log", &format!("⚠ \"{}\" failed: {}", seed, e)); }
        }
    }

    all_results
}

// ── KDP Paste Section Renderer ─────────────────────────────────────────────────

/// Renders the "KDP Metadata — Ready to Paste" section that mirrors the KDP
/// website's actual input layout. Categories are listed one per line with blank
/// line spacing; keywords are arranged in a 2-column table (Fields 1–4 left,
/// Fields 5–7 right) producing 4 rows where the 4th row's right cell is empty.
fn render_kdp_paste_section(
    kindle_categories: &[String],
    print_categories: &[String],
    keywords: &[db::KdpKeywordEntry],
) -> String {
    let json = serde_json::json!({
        "schema": "kdp_paste_v1",
        "kindle_categories": kindle_categories.iter().take(3).collect::<Vec<_>>(),
        "print_categories": print_categories.iter().take(3).collect::<Vec<_>>(),
        "keywords": keywords.iter().map(|k| &k.string).collect::<Vec<_>>(),
    });
    json.to_string()
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── derive_keyword_seeds tests ────────────────────────────────────────

    #[test]
    fn test_single_word_genre() {
        let seeds = derive_keyword_seeds("Romance", &[]);
        assert_eq!(seeds, vec!["romance"]);
    }

    #[test]
    fn test_three_word_genre() {
        let seeds = derive_keyword_seeds(
            "Christian Historical Fiction with Suspense",
            &[],
        );
        assert_eq!(seeds, vec!["christian historical fiction"]);
    }

    #[test]
    fn test_empty_categories() {
        let seeds = derive_keyword_seeds("Dark Fantasy", &[]);
        assert_eq!(seeds, vec!["dark fantasy"]);
    }

    #[test]
    fn test_categories_produce_leaf_seeds() {
        let cats = vec![
            "Kindle Store > Kindle eBooks > Mystery > Historical".to_string(),
            "Books > Literature & Fiction > Religious".to_string(),
        ];
        let seeds = derive_keyword_seeds("Christian Mystery", &cats);
        assert_eq!(seeds.len(), 3);
        assert_eq!(seeds[0], "christian mystery");
        assert_eq!(seeds[1], "historical");
        assert_eq!(seeds[2], "religious");
    }

    #[test]
    fn test_duplicate_seeds_deduplication() {
        // Genre seed = "historical fiction" and category leaf = "Historical Fiction"
        let cats = vec![
            "Books > Historical Fiction".to_string(),
        ];
        let seeds = derive_keyword_seeds("Historical Fiction Reimagined", &cats);
        // "historical fiction" from genre, "historical fiction" from category leaf — should deduplicate
        assert_eq!(seeds, vec!["historical fiction reimagined", "historical fiction"]);
    }

    #[test]
    fn test_duplicate_seeds_case_insensitive() {
        // Genre = "romance" (1 word), category leaf = "Romance"
        let cats = vec![
            "Kindle Store > Kindle eBooks > Romance".to_string(),
        ];
        let seeds = derive_keyword_seeds("Romance", &cats);
        // Both would be "romance" — dedup leaves just one
        assert_eq!(seeds, vec!["romance"]);
    }

    // ── aggregate_keyword_results tests ───────────────────────────────────

    #[test]
    fn test_aggregate_empty_input() {
        let results = aggregate_keyword_results(vec![]);
        assert!(results.is_empty());
    }

    #[test]
    fn test_aggregate_single_seed() {
        let results = aggregate_keyword_results(vec![
            ("romance", vec![
                KeywordResult { keyword: "dark romance".into(), searches: "5,000".into(), competition: "Low".into(), estimated_earnings: "$100".into() },
                KeywordResult { keyword: "clean romance".into(), searches: "3,000".into(), competition: "Med".into(), estimated_earnings: "$50".into() },
            ]),
        ]);
        assert_eq!(results.len(), 2);
        // Sorted descending by searches
        assert_eq!(results[0].keyword, "dark romance");
        assert_eq!(results[1].keyword, "clean romance");
    }

    #[test]
    fn test_aggregate_overlapping_keywords_keeps_higher() {
        let results = aggregate_keyword_results(vec![
            ("seed1", vec![
                KeywordResult { keyword: "historical fiction".into(), searches: "2,000".into(), competition: "Low".into(), estimated_earnings: "$80".into() },
                KeywordResult { keyword: "clean romance".into(), searches: "1,000".into(), competition: "Med".into(), estimated_earnings: "$40".into() },
            ]),
            ("seed2", vec![
                KeywordResult { keyword: "Historical Fiction".into(), searches: "8,500".into(), competition: "High".into(), estimated_earnings: "$200".into() },
                KeywordResult { keyword: "mystery thriller".into(), searches: "6,000".into(), competition: "Low".into(), estimated_earnings: "$150".into() },
            ]),
        ]);
        // "historical fiction" appears twice — keep the one with 8,500
        let hf = results.iter().find(|r| r.keyword.to_lowercase() == "historical fiction").unwrap();
        assert_eq!(hf.searches, "8,500");
        // Total unique keywords: 3
        assert_eq!(results.len(), 3);
        // Sorted descending: 8500, 6000, 1000
        assert_eq!(parse_searches(&results[0].searches), 8500);
        assert_eq!(parse_searches(&results[1].searches), 6000);
        assert_eq!(parse_searches(&results[2].searches), 1000);
    }

    #[test]
    fn test_aggregate_overlapping_keywords_lower_value_doesnt_replace() {
        let results = aggregate_keyword_results(vec![
            ("seed1", vec![
                KeywordResult { keyword: "Fantasy".into(), searches: "10,000".into(), competition: "High".into(), estimated_earnings: "$300".into() },
            ]),
            ("seed2", vec![
                KeywordResult { keyword: "fantasy".into(), searches: "5,000".into(), competition: "Low".into(), estimated_earnings: "$100".into() },
            ]),
        ]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].searches, "10,000");
    }

    // ── parse_searches tests ──────────────────────────────────────────────

    #[test]
    fn test_parse_searches_with_commas() {
        assert_eq!(parse_searches("12,345"), 12345);
    }

    #[test]
    fn test_parse_searches_plain_number() {
        assert_eq!(parse_searches("500"), 500);
    }

    #[test]
    fn test_parse_searches_invalid() {
        assert_eq!(parse_searches("N/A"), 0);
        assert_eq!(parse_searches(""), 0);
    }

    // ── render_kdp_paste_section tests ────────────────────────────────────

    fn make_keyword(s: &str) -> db::KdpKeywordEntry {
        db::KdpKeywordEntry {
            string: s.to_string(),
            chars: s.len() as i64,
            rationale: "test rationale".to_string(),
        }
    }

    #[test]
    fn test_kdp_paste_section_full_7_keywords() {
        let kindle = vec![
            "Kindle Books > Literature & Fiction > Religious & Inspirational > Christian".to_string(),
            "Kindle Books > Literature & Fiction > Historical Fiction > Religious".to_string(),
            "Kindle Books > Mystery, Thriller & Suspense > Mystery > Historical".to_string(),
        ];
        let print = vec![
            "Books > Literature & Fiction > Genre Fiction > Religious & Inspirational > Christian".to_string(),
            "Books > Literature & Fiction > Genre Fiction > Historical > Mystery".to_string(),
            "Books > Mystery, Thriller & Suspense > Mystery > Historical".to_string(),
        ];
        let keywords: Vec<db::KdpKeywordEntry> = vec![
            make_keyword("christian historical mystery"),
            make_keyword("first century faith novel"),
            make_keyword("early church fiction"),
            make_keyword("roman empire christian"),
            make_keyword("ancient rome fiction"),
            make_keyword("biblical thriller"),
            make_keyword("religious suspense clean"),
        ];

        let output = render_kdp_paste_section(&kindle, &print, &keywords);

        // Check section header
        assert!(output.contains("## KDP Metadata — Ready to Paste"));

        // Check Kindle categories subsection
        assert!(output.contains("### Categories (Kindle eBook)"));
        assert!(output.contains("Kindle Books > Literature & Fiction > Religious & Inspirational > Christian"));
        assert!(output.contains("Kindle Books > Literature & Fiction > Historical Fiction > Religious"));
        assert!(output.contains("Kindle Books > Mystery, Thriller & Suspense > Mystery > Historical"));

        // Check Paperback categories subsection
        assert!(output.contains("### Categories (Paperback)"));
        assert!(output.contains("Books > Literature & Fiction > Genre Fiction > Religious & Inspirational > Christian"));

        // Check keywords table
        assert!(output.contains("| Field 1–4 | Field 5–7 |"));
        assert!(output.contains("|-----------|-----------|"));
        assert!(output.contains("| christian historical mystery | ancient rome fiction |"));
        assert!(output.contains("| first century faith novel | biblical thriller |"));
        assert!(output.contains("| early church fiction | religious suspense clean |"));
        assert!(output.contains("| roman empire christian |  |"));
    }

    #[test]
    fn test_kdp_paste_section_fewer_than_3_categories() {
        let kindle = vec![
            "Kindle Books > Mystery > Historical".to_string(),
        ];
        let print = vec![
            "Books > Mystery > Historical".to_string(),
            "Books > Fiction > General".to_string(),
        ];
        let keywords = vec![
            make_keyword("kw1"),
            make_keyword("kw2"),
            make_keyword("kw3"),
            make_keyword("kw4"),
            make_keyword("kw5"),
            make_keyword("kw6"),
            make_keyword("kw7"),
        ];

        let output = render_kdp_paste_section(&kindle, &print, &keywords);

        // Only 1 Kindle category should appear
        assert!(output.contains("Kindle Books > Mystery > Historical"));
        // Both print categories
        assert!(output.contains("Books > Mystery > Historical"));
        assert!(output.contains("Books > Fiction > General"));
        // Keywords still have 4 rows
        assert!(output.contains("| kw1 | kw5 |"));
        assert!(output.contains("| kw3 | kw7 |"));
        assert!(output.contains("| kw4 |  |"));
    }

    #[test]
    fn test_kdp_paste_section_fewer_than_7_keywords() {
        let kindle = vec!["Cat A".to_string()];
        let print = vec!["Cat B".to_string()];
        let keywords = vec![
            make_keyword("alpha"),
            make_keyword("beta"),
            make_keyword("gamma"),
        ];

        let output = render_kdp_paste_section(&kindle, &print, &keywords);

        // 4 rows: first 3 have left filled, only none in right except nothing beyond index 4
        assert!(output.contains("| alpha |  |"));
        assert!(output.contains("| beta |  |"));
        assert!(output.contains("| gamma |  |"));
        assert!(output.contains("|  |  |"));
    }

    #[test]
    fn test_kdp_paste_section_empty_inputs() {
        let output = render_kdp_paste_section(&[], &[], &[]);

        assert!(output.contains("## KDP Metadata — Ready to Paste"));
        assert!(output.contains("### Categories (Kindle eBook)"));
        assert!(output.contains("### Categories (Paperback)"));
        assert!(output.contains("### Keywords"));
        // 4 rows all empty
        assert!(output.contains("|  |  |"));
    }

    #[test]
    fn test_kdp_paste_section_more_than_3_categories_truncates() {
        let kindle = vec![
            "Cat 1".to_string(),
            "Cat 2".to_string(),
            "Cat 3".to_string(),
            "Cat 4".to_string(), // should be excluded
        ];
        let print = vec![
            "Print 1".to_string(),
            "Print 2".to_string(),
            "Print 3".to_string(),
            "Print 4".to_string(), // should be excluded
        ];
        let keywords = vec![make_keyword("kw1")];

        let output = render_kdp_paste_section(&kindle, &print, &keywords);

        assert!(output.contains("Cat 1"));
        assert!(output.contains("Cat 2"));
        assert!(output.contains("Cat 3"));
        assert!(!output.contains("Cat 4"));
        assert!(output.contains("Print 1"));
        assert!(output.contains("Print 2"));
        assert!(output.contains("Print 3"));
        assert!(!output.contains("Print 4"));
    }

    #[test]
    fn test_kdp_paste_section_categories_have_blank_line_spacing() {
        let kindle = vec![
            "Category A".to_string(),
            "Category B".to_string(),
        ];
        let print = vec!["Print X".to_string()];
        let keywords = vec![make_keyword("kw")];

        let output = render_kdp_paste_section(&kindle, &print, &keywords);

        // Each category should be followed by a blank line
        assert!(output.contains("Category A\n\n"));
        assert!(output.contains("Category B\n\n"));
        assert!(output.contains("Print X\n\n"));
    }

    // Feature: consolidate-pipeline, Property 2: Keyword aggregation deduplication keeps higher searches
    // **Validates: Requirements 2.2**
    mod prop_aggregate_dedup {
        use super::*;
        use proptest::prelude::*;

        /// Generate a comma-formatted searches string from a u64 value
        fn format_searches(n: u64) -> String {
            let s = n.to_string();
            let bytes = s.as_bytes();
            let mut result = String::new();
            for (i, &b) in bytes.iter().enumerate() {
                if i > 0 && (bytes.len() - i) % 3 == 0 {
                    result.push(',');
                }
                result.push(b as char);
            }
            result
        }

        /// Strategy for generating a KeywordResult with a given keyword and searches value
        fn keyword_result_strategy() -> impl Strategy<Value = (String, u64, KeywordResult)> {
            (
                "[a-z ]{1,20}",      // keyword text
                1u64..1_000_000,     // searches value (numeric)
                "[a-zA-Z]{1,5}",     // competition
                "[a-zA-Z0-9$]{1,5}", // estimated_earnings
            )
                .prop_map(|(keyword, searches_val, competition, earnings)| {
                    let searches_str = format_searches(searches_val);
                    let kr = KeywordResult {
                        keyword: keyword.clone(),
                        searches: searches_str,
                        competition,
                        estimated_earnings: earnings,
                    };
                    (keyword.to_lowercase(), searches_val, kr)
                })
        }

        /// Strategy for a seed group: a seed name + 1-10 keyword results
        fn seed_group_strategy() -> impl Strategy<Value = (String, Vec<(String, u64, KeywordResult)>)> {
            (
                "[a-z]{3,10}",  // seed name
                proptest::collection::vec(keyword_result_strategy(), 1..=10),
            )
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(200))]

            #[test]
            fn prop_aggregate_dedup_keeps_max_searches(
                seed_groups in proptest::collection::vec(seed_group_strategy(), 2..=5)
            ) {
                use std::collections::HashMap;

                // Build input for aggregate_keyword_results
                let mut input: Vec<(String, Vec<KeywordResult>)> = Vec::new();
                // Track expected max searches per keyword (case-insensitive)
                let mut expected_max: HashMap<String, u64> = HashMap::new();

                for (seed_name, keyword_entries) in &seed_groups {
                    let mut results: Vec<KeywordResult> = Vec::new();
                    for (lower_kw, searches_val, kr) in keyword_entries {
                        results.push(kr.clone());
                        let entry = expected_max.entry(lower_kw.clone()).or_insert(0);
                        if *searches_val > *entry {
                            *entry = *searches_val;
                        }
                    }
                    input.push((seed_name.clone(), results));
                }

                // Convert to the expected signature: Vec<(&str, Vec<KeywordResult>)>
                let input_refs: Vec<(&str, Vec<KeywordResult>)> = input
                    .iter()
                    .map(|(s, v)| (s.as_str(), v.clone()))
                    .collect();

                let output = aggregate_keyword_results(input_refs);

                // Property 1: Exactly one entry per unique keyword (case-insensitive)
                let mut seen_keywords: HashMap<String, usize> = HashMap::new();
                for r in &output {
                    let key = r.keyword.to_lowercase();
                    *seen_keywords.entry(key).or_insert(0) += 1;
                }
                for (kw, count) in &seen_keywords {
                    prop_assert_eq!(
                        *count, 1,
                        "Keyword '{}' appeared {} times in output, expected 1",
                        kw, count
                    );
                }
                prop_assert_eq!(
                    output.len(),
                    expected_max.len(),
                    "Output count {} != expected unique keywords {}",
                    output.len(),
                    expected_max.len()
                );

                // Property 2: Each output entry has the maximum searches value
                for r in &output {
                    let key = r.keyword.to_lowercase();
                    let actual_searches = parse_searches(&r.searches);
                    let expected = expected_max.get(&key).copied().unwrap_or(0);
                    prop_assert_eq!(
                        actual_searches, expected,
                        "Keyword '{}': got searches={}, expected max={}",
                        key, actual_searches, expected
                    );
                }

                // Property 3: Output is sorted descending by parsed searches
                for i in 1..output.len() {
                    let prev = parse_searches(&output[i - 1].searches);
                    let curr = parse_searches(&output[i].searches);
                    prop_assert!(
                        prev >= curr,
                        "Output not sorted descending at index {}: {} < {}",
                        i, prev, curr
                    );
                }
            }
        }
    }

    // Feature: consolidate-pipeline, Property 4: Discovery keyword output is structurally valid
    // **Validates: Requirements 4.1, 4.2**
    mod prop_discovery_keyword_output {
        use super::*;
        use proptest::prelude::*;

        /// Strategy to generate a single discovery keyword entry as a JSON value.
        /// Phrases always start with at least one alpha char to guarantee non-empty after trim.
        fn discovery_entry_strategy() -> impl Strategy<Value = serde_json::Value> {
            (
                "[a-z][a-z ]{1,29}",   // phrase: starts with alpha, guaranteed non-empty after trim
                "[a-zA-Z][a-zA-Z ]{4,39}", // rationale suffix: starts with alpha
            )
                .prop_map(|(phrase, rationale_suffix)| {
                    serde_json::json!({
                        "phrase": phrase.trim(),
                        "rationale": format!("AI-reasoned: {}", rationale_suffix.trim())
                    })
                })
        }

        /// Strategy to generate a valid discovery keywords JSON response (exactly 10 items)
        fn discovery_response_strategy() -> impl Strategy<Value = String> {
            proptest::collection::vec(discovery_entry_strategy(), 10..=10)
                .prop_map(|entries| {
                    let obj = serde_json::json!({ "keywords": entries });
                    serde_json::to_string(&obj).unwrap()
                })
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(200))]

            #[test]
            fn prop_discovery_keyword_output_is_structurally_valid(
                json_response in discovery_response_strategy()
            ) {
                // Parse the JSON the same way generate_discovery_keywords does
                let v: serde_json::Value = serde_json::from_str(&json_response)
                    .expect("Generated JSON should be valid");

                let keywords = v["keywords"].as_array()
                    .expect("Should have a keywords array");

                let entries: Vec<db::DiscoveryKeywordEntry> = keywords.iter().map(|kw| {
                    db::DiscoveryKeywordEntry {
                        phrase:    kw["phrase"].as_str().unwrap_or("").to_string(),
                        rationale: kw["rationale"].as_str().unwrap_or("AI-reasoned").to_string(),
                    }
                }).collect();

                // Property: Parsed output contains exactly 10 entries
                prop_assert_eq!(
                    entries.len(), 10,
                    "Expected exactly 10 entries, got {}", entries.len()
                );

                // Property: Each phrase is non-empty
                for (i, entry) in entries.iter().enumerate() {
                    prop_assert!(
                        !entry.phrase.is_empty(),
                        "Entry {} has empty phrase", i
                    );
                }

                // Property: Each rationale contains "AI-reasoned" (case-insensitive)
                for (i, entry) in entries.iter().enumerate() {
                    prop_assert!(
                        entry.rationale.to_lowercase().contains("ai-reasoned"),
                        "Entry {} rationale '{}' does not contain 'ai-reasoned'",
                        i, entry.rationale
                    );
                }
            }
        }
    }

    // Feature: consolidate-pipeline, Property 5: Combined report contains all required sections
    // **Validates: Requirements 5.8, 8.5**
    mod prop_combined_report_ordering {
        use super::*;
        use proptest::prelude::*;

        /// Strategy for generating non-empty section content strings (1-200 chars)
        /// that won't contain markdown headers which could confuse section detection.
        fn section_content_strategy() -> impl Strategy<Value = String> {
            "[a-zA-Z0-9 ,.:;!?()]{1,200}"
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(100))]

            #[test]
            fn prop_combined_report_section_ordering(
                kdp_content in section_content_strategy(),
                genre_content in section_content_strategy(),
                categories_content in section_content_strategy(),
                bisac_content in section_content_strategy(),
                keywords_content in section_content_strategy(),
                discovery_content in section_content_strategy(),
                positioning_content in section_content_strategy(),
            ) {
                // Build section strings that include the required section headers
                let kdp_paste = format!("## KDP Metadata\n\n{}", kdp_content);
                let genre_ranking = format!("## Genre Ranking\n\n{}", genre_content);
                let kdp_categories = format!("## KDP Categories\n\n{}", categories_content);
                let bisac = format!("## BISAC Classification\n\n{}", bisac_content);
                let kdp_keywords = format!("## KDP Keywords\n\n{}", keywords_content);
                let discovery_keywords = format!("## Discovery Keywords\n\n{}", discovery_content);
                let positioning = format!("## Positioning Context\n\n{}", positioning_content);

                let report = render_combined_report(
                    &kdp_paste,
                    &genre_ranking,
                    &kdp_categories,
                    &bisac,
                    &kdp_keywords,
                    &discovery_keywords,
                    &positioning,
                );

                // Assert: output starts with "# Full Analysis Report"
                prop_assert!(
                    report.starts_with("# Full Analysis Report"),
                    "Report must start with '# Full Analysis Report', got: '{}'",
                    &report[..report.len().min(50)]
                );

                // Find positions of each section header
                let pos_kdp = report.find("KDP Metadata");
                let pos_genre = report.find("Genre Ranking");
                let pos_categories = report.find("KDP Categories");
                let pos_bisac = report.find("BISAC Classification");
                let pos_keywords = report.find("KDP Keywords");
                let pos_discovery = report.find("Discovery Keywords");
                let pos_positioning = report.find("Positioning Context");

                // Assert all sections are present
                prop_assert!(pos_kdp.is_some(), "Report missing 'KDP Metadata' section");
                prop_assert!(pos_genre.is_some(), "Report missing 'Genre Ranking' section");
                prop_assert!(pos_categories.is_some(), "Report missing 'KDP Categories' section");
                prop_assert!(pos_bisac.is_some(), "Report missing 'BISAC Classification' section");
                prop_assert!(pos_keywords.is_some(), "Report missing 'KDP Keywords' section");
                prop_assert!(pos_discovery.is_some(), "Report missing 'Discovery Keywords' section");
                prop_assert!(pos_positioning.is_some(), "Report missing 'Positioning Context' section");

                let pos_kdp = pos_kdp.unwrap();
                let pos_genre = pos_genre.unwrap();
                let pos_categories = pos_categories.unwrap();
                let pos_bisac = pos_bisac.unwrap();
                let pos_keywords = pos_keywords.unwrap();
                let pos_discovery = pos_discovery.unwrap();
                let pos_positioning = pos_positioning.unwrap();

                // Assert ordering: KDP Metadata < Genre Ranking < KDP Categories < BISAC Classification < KDP Keywords < Discovery Keywords < Positioning Context
                prop_assert!(
                    pos_kdp < pos_genre,
                    "'KDP Metadata' (pos {}) must appear before 'Genre Ranking' (pos {})",
                    pos_kdp, pos_genre
                );
                prop_assert!(
                    pos_genre < pos_categories,
                    "'Genre Ranking' (pos {}) must appear before 'KDP Categories' (pos {})",
                    pos_genre, pos_categories
                );
                prop_assert!(
                    pos_categories < pos_bisac,
                    "'KDP Categories' (pos {}) must appear before 'BISAC Classification' (pos {})",
                    pos_categories, pos_bisac
                );
                prop_assert!(
                    pos_bisac < pos_keywords,
                    "'BISAC Classification' (pos {}) must appear before 'KDP Keywords' (pos {})",
                    pos_bisac, pos_keywords
                );
                prop_assert!(
                    pos_keywords < pos_discovery,
                    "'KDP Keywords' (pos {}) must appear before 'Discovery Keywords' (pos {})",
                    pos_keywords, pos_discovery
                );
                prop_assert!(
                    pos_discovery < pos_positioning,
                    "'Discovery Keywords' (pos {}) must appear before 'Positioning Context' (pos {})",
                    pos_discovery, pos_positioning
                );

                // Assert all sections are separated by "---"
                prop_assert!(
                    report.contains("---"),
                    "Report must contain '---' separators between sections"
                );

                // Check that "---" appears between each pair of adjacent sections
                let between_kdp_genre = &report[pos_kdp..pos_genre];
                prop_assert!(
                    between_kdp_genre.contains("---"),
                    "Missing '---' separator between KDP Metadata and Genre Ranking"
                );

                let between_genre_categories = &report[pos_genre..pos_categories];
                prop_assert!(
                    between_genre_categories.contains("---"),
                    "Missing '---' separator between Genre Ranking and KDP Categories"
                );

                let between_categories_bisac = &report[pos_categories..pos_bisac];
                prop_assert!(
                    between_categories_bisac.contains("---"),
                    "Missing '---' separator between KDP Categories and BISAC Classification"
                );

                let between_bisac_keywords = &report[pos_bisac..pos_keywords];
                prop_assert!(
                    between_bisac_keywords.contains("---"),
                    "Missing '---' separator between BISAC Classification and KDP Keywords"
                );

                let between_keywords_discovery = &report[pos_keywords..pos_discovery];
                prop_assert!(
                    between_keywords_discovery.contains("---"),
                    "Missing '---' separator between KDP Keywords and Discovery Keywords"
                );

                let between_discovery_positioning = &report[pos_discovery..pos_positioning];
                prop_assert!(
                    between_discovery_positioning.contains("---"),
                    "Missing '---' separator between Discovery Keywords and Positioning Context"
                );
            }
        }
    }

    // Feature: consolidate-pipeline, Property 6: KDP paste section mirrors website input layout
    // **Validates: Requirements 8.1, 8.2, 8.3, 8.4**
    mod prop_kdp_paste_layout {
        use super::*;
        use proptest::prelude::*;

        /// Generate a non-empty string with no newlines (for category names)
        fn category_strategy() -> impl Strategy<Value = String> {
            "[a-zA-Z0-9 >&,]{1,40}"
                .prop_filter("must not contain newlines", |s| !s.contains('\n') && !s.is_empty())
        }

        /// Generate 1-3 category strings
        fn categories_vec_strategy() -> impl Strategy<Value = Vec<String>> {
            proptest::collection::vec(category_strategy(), 1..=3)
        }

        /// Generate a non-empty keyword string with no pipes or newlines
        fn keyword_string_strategy() -> impl Strategy<Value = String> {
            "[a-zA-Z0-9 ]{1,30}"
                .prop_filter("must not contain pipes or newlines", |s| {
                    !s.contains('|') && !s.contains('\n') && !s.is_empty()
                })
        }

        /// Generate exactly 7 KdpKeywordEntry items
        fn keywords_strategy() -> impl Strategy<Value = Vec<db::KdpKeywordEntry>> {
            proptest::collection::vec(keyword_string_strategy(), 7..=7)
                .prop_map(|strings| {
                    strings.into_iter().map(|s| {
                        let chars = s.len() as i64;
                        db::KdpKeywordEntry {
                            string: s,
                            chars,
                            rationale: "test".to_string(),
                        }
                    }).collect()
                })
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(200))]

            #[test]
            fn prop_kdp_paste_section_layout(
                kindle_cats in categories_vec_strategy(),
                print_cats in categories_vec_strategy(),
                keywords in keywords_strategy(),
            ) {
                let output = render_kdp_paste_section(&kindle_cats, &print_cats, &keywords);

                // Assert required section headers are present
                prop_assert!(output.contains("## KDP Metadata — Ready to Paste"),
                    "Missing main header");
                prop_assert!(output.contains("### Categories (Kindle eBook)"),
                    "Missing Kindle categories header");
                prop_assert!(output.contains("### Categories (Paperback)"),
                    "Missing Paperback categories header");
                prop_assert!(output.contains("### Keywords"),
                    "Missing Keywords header");

                // Each Kindle category (up to 3) appears in the output
                for cat in kindle_cats.iter().take(3) {
                    prop_assert!(output.contains(cat.as_str()),
                        "Kindle category '{}' missing from output", cat);
                }

                // Each Paperback category (up to 3) appears in the output
                for cat in print_cats.iter().take(3) {
                    prop_assert!(output.contains(cat.as_str()),
                        "Paperback category '{}' missing from output", cat);
                }

                // Table header present
                prop_assert!(output.contains("| Field 1\u{2013}4 | Field 5\u{2013}7 |"),
                    "Missing table header '| Field 1–4 | Field 5–7 |'");

                // Keywords 1-4 appear in left column, keywords 5-7 in right column
                // Row format: "| left | right |"
                for i in 0..4 {
                    let left = &keywords[i].string;
                    let right_kw = keywords.get(i + 4);
                    let right = right_kw.map(|k| k.string.as_str()).unwrap_or("");
                    let expected_row = format!("| {} | {} |", left, right);
                    prop_assert!(output.contains(&expected_row),
                        "Missing expected row: '{}'", expected_row);
                }

                // 4th row (i=3): right cell should be empty since keywords[3+4]=keywords[7] doesn't exist
                let fourth_row = format!("| {} |  |", keywords[3].string);
                prop_assert!(output.contains(&fourth_row),
                    "4th row's right cell should be empty: expected '{}'", fourth_row);

                // Categories are separated by blank lines (each category followed by \n\n)
                for cat in kindle_cats.iter().take(3) {
                    let pattern = format!("{}\n\n", cat);
                    prop_assert!(output.contains(&pattern),
                        "Kindle category '{}' not followed by blank line", cat);
                }
                for cat in print_cats.iter().take(3) {
                    let pattern = format!("{}\n\n", cat);
                    prop_assert!(output.contains(&pattern),
                        "Paperback category '{}' not followed by blank line", cat);
                }
            }
        }
    }

    // Feature: consolidate-pipeline, Property 3: KDP keyword output is structurally valid
    // **Validates: Requirements 3.1, 3.3, 3.5**
    mod prop_kdp_keyword_output {
        use super::*;
        use proptest::prelude::*;

        /// Strategy for generating a KeywordResult with arbitrary content
        fn keyword_result_strategy() -> impl Strategy<Value = KeywordResult> {
            (
                "[a-z ]{1,30}",          // keyword text
                "[0-9,]{1,7}",           // searches (formatted string)
                "[A-Za-z]{1,10}",        // competition
                "[A-Za-z0-9$. ]{1,10}",  // estimated_earnings
            )
                .prop_map(|(keyword, searches, competition, estimated_earnings)| {
                    KeywordResult { keyword, searches, competition, estimated_earnings }
                })
        }

        /// Strategy for generating a Vec<KeywordResult> with 1-100 items
        fn keyword_pool_strategy() -> impl Strategy<Value = Vec<KeywordResult>> {
            proptest::collection::vec(keyword_result_strategy(), 1..=100)
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(200))]

            #[test]
            fn prop_format_keyword_pool_table_is_valid_markdown(
                pool in keyword_pool_strategy()
            ) {
                let table = format_keyword_pool_table(&pool);
                let lines: Vec<&str> = table.lines().collect();

                // Must have a header row with correct columns
                prop_assert!(lines.len() >= 2, "Table must have at least header + separator");

                let header = lines[0];
                prop_assert!(header.contains("Keyword"),
                    "Header must contain 'Keyword', got: {}", header);
                prop_assert!(header.contains("Monthly Searches"),
                    "Header must contain 'Monthly Searches', got: {}", header);
                prop_assert!(header.contains("Competition"),
                    "Header must contain 'Competition', got: {}", header);
                prop_assert!(header.contains("Est. Earnings"),
                    "Header must contain 'Est. Earnings', got: {}", header);

                // Second line is the separator row
                let separator = lines[1];
                prop_assert!(separator.contains("---"),
                    "Second row must be a markdown separator, got: {}", separator);

                // Data rows: at most 50 (capped)
                let data_rows = lines.len() - 2;  // subtract header + separator
                let expected_rows = pool.len().min(50);
                prop_assert_eq!(data_rows, expected_rows,
                    "Expected {} data rows (pool size {} capped at 50), got {}",
                    expected_rows, pool.len(), data_rows);

                // Each keyword from input (up to 50) appears in the table
                for r in pool.iter().take(50) {
                    prop_assert!(table.contains(&r.keyword),
                        "Keyword '{}' should appear in the table", r.keyword);
                }

                // Keywords beyond 50 should NOT appear (if pool > 50)
                if pool.len() > 50 {
                    // Check that the 51st keyword's data row is not present
                    // (only if the keyword doesn't also appear in the first 50 by coincidence)
                    let first_50_keywords: Vec<&str> = pool.iter().take(50).map(|r| r.keyword.as_str()).collect();
                    for r in pool.iter().skip(50) {
                        if !first_50_keywords.contains(&r.keyword.as_str()) {
                            // Build the exact data row that would appear
                            let expected_row = format!("| {} | {} | {} | {} |",
                                r.keyword, r.searches, r.competition, r.estimated_earnings);
                            prop_assert!(!table.contains(&expected_row),
                                "Keyword '{}' beyond 50-row cap should not appear as a data row", r.keyword);
                        }
                    }
                }

                // Each data row starts and ends with '|' (valid markdown table row)
                for (i, line) in lines.iter().enumerate().skip(2) {
                    prop_assert!(line.starts_with('|') && line.ends_with('|'),
                        "Data row {} should start and end with '|', got: {}", i, line);
                }
            }
        }
    }

    // Feature: consolidate-pipeline, Property 1: Seed derivation produces valid seeds
    // **Validates: Requirements 1.1, 1.2, 1.3, 1.4, 1.5**
    mod prop_seed_derivation {
        use super::*;
        use proptest::prelude::*;

        /// Generate a random word (1-12 lowercase alpha chars)
        fn word_strategy() -> impl Strategy<Value = String> {
            "[a-z]{1,12}".prop_map(|s| s)
        }

        /// Generate an industry_ebook string with 1-10 whitespace-separated words
        fn industry_ebook_strategy() -> impl Strategy<Value = String> {
            prop::collection::vec(word_strategy(), 1..=10)
                .prop_map(|words| words.join(" "))
        }

        /// Generate a category path with 1-4 segments separated by " > "
        fn category_path_strategy() -> impl Strategy<Value = String> {
            prop::collection::vec(word_strategy(), 1..=4)
                .prop_map(|segments| segments.join(" > "))
        }

        /// Generate 0-2 category paths
        fn categories_strategy() -> impl Strategy<Value = Vec<String>> {
            prop::collection::vec(category_path_strategy(), 0..=2)
        }

        proptest! {
            #[test]
            fn prop_seed_derivation_produces_valid_seeds(
                industry_ebook in industry_ebook_strategy(),
                categories in categories_strategy(),
            ) {
                let seeds = derive_keyword_seeds(&industry_ebook, &categories);

                // Output has 1-3 elements
                prop_assert!(!seeds.is_empty(), "Seeds must not be empty for non-empty input");
                prop_assert!(seeds.len() <= 3, "Seeds must have at most 3 elements, got {}", seeds.len());

                // All elements are lowercase
                for seed in &seeds {
                    prop_assert_eq!(seed, &seed.to_lowercase(), "Seed '{}' is not lowercase", seed);
                }

                // No case-insensitive duplicates
                let lowers: Vec<String> = seeds.iter().map(|s| s.to_lowercase()).collect();
                for i in 0..lowers.len() {
                    for j in (i + 1)..lowers.len() {
                        prop_assert_ne!(&lowers[i], &lowers[j],
                            "Duplicate seeds found: '{}' at indices {} and {}", lowers[i], i, j);
                    }
                }

                // First seed is first 2-3 words of industry_ebook lowercased
                let words: Vec<&str> = industry_ebook.split_whitespace().collect();
                let expected_first = words[..words.len().min(3)].join(" ").to_lowercase();
                prop_assert_eq!(&seeds[0], &expected_first,
                    "First seed should be first 2-3 words of industry_ebook lowercased");

                // Additional seeds are leaf segments of categories (text after last ">")
                let mut expected_additional: Vec<String> = Vec::new();
                for cat in categories.iter().take(2) {
                    let leaf = cat.rsplit('>').next().unwrap_or("").trim().to_lowercase();
                    if !leaf.is_empty() {
                        expected_additional.push(leaf);
                    }
                }

                // Verify each additional seed matches the expected leaf
                // (accounting for deduplication — if a leaf duplicates seed[0] or a prior leaf, it's removed)
                let mut expected_all: Vec<String> = vec![expected_first.clone()];
                for leaf in &expected_additional {
                    if !expected_all.iter().any(|s| s.to_lowercase() == leaf.to_lowercase()) {
                        expected_all.push(leaf.clone());
                    }
                }

                prop_assert_eq!(seeds.len(), expected_all.len(),
                    "Seeds length mismatch: got {:?}, expected {:?}", seeds, expected_all);

                for (i, (actual, expected)) in seeds.iter().zip(expected_all.iter()).enumerate() {
                    prop_assert_eq!(actual, expected,
                        "Seed at index {} mismatch: got '{}', expected '{}'", i, actual, expected);
                }
            }
        }
    }
}
