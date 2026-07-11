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
    pub has_folder:          bool,
    pub summary_count:       usize,
    pub has_genre_data:      bool,
    pub has_full_report:     bool,
    pub has_keywords:        bool,
    pub has_pr_keywords:     bool,
    pub has_competition:     bool,
    pub has_categories:      bool,
    pub has_genre_ranking:   bool,
    pub has_mapped_verified: bool,
}

#[tauri::command]
pub async fn check_analysis_state(app: AppHandle, folder: String) -> AnalysisState {
    tokio::task::spawn_blocking(move || {
        let folder_path = PathBuf::from(&folder);
        let database    = app.state::<db::Db>();
        let conn        = database.0.lock().unwrap();

        AnalysisState {
            has_folder:          folder_path.exists(),
            summary_count:       db::chapter_summary_count(&conn, &folder) as usize,
            has_genre_data:      db::load_genre_data(&conn, &folder).is_some(),
            has_full_report:     db::get_document(&conn, &folder, "full_report").is_some(),
            has_keywords:        db::load_kdp_keywords(&conn, &folder).is_some(),
            has_pr_keywords:     !db::load_pr_keywords(&conn, &folder).is_empty(),
            has_competition:     db::get_document(&conn, &folder, "competition_report").is_some(),
            has_categories:      db::has_category_results(&conn, &folder),
            has_genre_ranking:   db::has_genre_rankings(&conn, &folder),
            has_mapped_verified: db::get_document(&conn, &folder, "mapped_categories").is_some(),
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
pub async fn verify_mapped_categories(app: AppHandle, db: tauri::State<'_, db::Db>, request: VerifyMappedRequest) -> GenreResult {
    let rankings = {
        let conn = db.0.lock().unwrap();
        match db::get_genre_rankings(&conn, &request.folder, &request.store) {
            Ok(r) => r,
            Err(e) => return err(&format!("Could not read rankings from database: {}", e)),
        }
    };

    if rankings.is_empty() {
        return err("No genre rankings found for this story. Run Rank Genres first.");
    }

    let mut paths: Vec<String> = Vec::new();
    for g in &rankings {
        for p in &g.kdp_paths {
            if !paths.contains(p) { paths.push(p.clone()); }
        }
    }

    if paths.is_empty() {
        return err("None of this story's ranked genres have a mapped KDP path yet. Run Category Finder to discover paths — they'll be saved to the database automatically.");
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
        return err(&result.error);
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

    GenreResult { success: true, report: result.markdown, error: String::new() }
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

// ── Rendering (structured DB data → markdown, always fresh, never cached-stale) ─

fn render_genre_analysis_md(g: &db::GenreDataRow) -> String {
    let now = chrono::Utc::now().format("%B %-d, %Y %H:%M UTC").to_string();
    let mut md = vec![
        "# Genre Analysis Report".to_string(),
        format!("Generated: {}", now),
        String::new(),
        "> **Note:** Classifications are based on AI training data. Verify KDP paths in Publisher Rocket.".to_string(),
        String::new(),
        "---".to_string(),
        String::new(),
        "## 1. Industry Genre Classification".to_string(),
        String::new(),
        "### Ebook".to_string(),
        format!("**{}**", g.industry_ebook),
        String::new(),
    ];
    if !g.comps_ebook.is_empty() {
        md.push("**Comparable titles:**".to_string());
        for c in &g.comps_ebook { md.push(format!("- {}", c)); }
        md.push(String::new());
    }
    md.push(format!("**Reader demographic:** {}", g.reader_demographic));
    md.push(String::new());
    md.push("### Print".to_string());
    md.push(format!("**{}**", g.industry_print));
    md.push(String::new());
    md.push(format!("**Bookstore shelving:** {}", g.bookstore_shelving));
    md.push(String::new());
    if !g.comps_print.is_empty() {
        md.push("**Comparable titles:**".to_string());
        for c in &g.comps_print { md.push(format!("- {}", c)); }
        md.push(String::new());
    }
    md.push("---".to_string());
    md.push(String::new());
    md.push("## 2. KDP Category Recommendations".to_string());
    md.push(String::new());
    md.push("### Kindle Ebook".to_string());
    for p in &g.kdp_ebook { md.push(format!("- `{}`", p)); }
    md.push(String::new());
    md.push("### KDP Print".to_string());
    for p in &g.kdp_print { md.push(format!("- `{}`", p)); }
    md.push(String::new());
    md.push("---".to_string());
    md.push(String::new());
    md.push("## 3. Genre Signals Summary".to_string());
    md.push(String::new());
    md.push(g.genre_signals.clone());
    md.push(String::new());
    md.push("---".to_string());
    md.push(String::new());
    md.push("## 4. Marketing Notes".to_string());
    md.push(String::new());
    for note in &g.marketing_notes { md.push(format!("- {}", note)); }
    md.push(String::new());
    md.join("\n")
}

fn render_full_report(g: &db::GenreDataRow, competition_done: bool) -> String {
    let now = chrono::Utc::now().format("%B %-d, %Y %H:%M UTC").to_string();
    let genre_report = render_genre_analysis_md(g);
    let footer = if competition_done {
        "> Run **Analyze Competition** to refresh Publisher Rocket market data, category stats,\n> competitor pricing, and cover analysis in this report."
    } else {
        "> Run **Analyze Competition** to add Publisher Rocket market data."
    };
    vec![
        "# Full Genre & Market Analysis".to_string(),
        format!("Generated: {}", now),
        String::new(), "---".to_string(), String::new(),
        genre_report,
        String::new(), "---".to_string(), String::new(),
        footer.to_string(),
        String::new(),
    ].join("\n")
}

fn render_kdp_keywords(entries: &[db::KdpKeywordEntry], strategy: &str, source_note: &str) -> String {
    let now = chrono::Utc::now().format("%B %-d, %Y %H:%M UTC").to_string();
    let mut md = vec![
        source_note.to_string(),
        String::new(),
        "# KDP Keyword Strings".to_string(),
        format!("Generated: {}", now),
        String::new(),
        "> These 7 strings are ready to paste directly into KDP's keyword fields.".to_string(),
        "> Each is 50 characters or fewer.".to_string(),
        String::new(),
        "---".to_string(),
        String::new(),
        "## Your 7 KDP Keyword Strings".to_string(),
        String::new(),
    ];
    for (i, kw) in entries.iter().enumerate() {
        let actual = kw.string.len() as i64;
        let flag = if actual > 50 { " ⚠️ OVER 50 CHARS — shorten before using" } else { "" };
        md.push(format!("**{}. `{}`**{}", i + 1, kw.string, flag));
        md.push(format!("*{} characters — {}*", kw.chars.max(actual), kw.rationale));
        md.push(String::new());
    }
    md.push("---".to_string());
    md.push(String::new());
    md.push("## Strategy".to_string());
    md.push(String::new());
    md.push(strategy.to_string());
    md.push(String::new());
    md.push("---".to_string());
    md.push(String::new());
    md.push("## How to Use".to_string());
    md.push(String::new());
    md.push("1. Go to KDP → Your Books → Edit eBook Details".to_string());
    md.push("2. Scroll to **Keywords** (7 fields)".to_string());
    md.push("3. Paste one string per field".to_string());
    md.push("4. Do NOT use commas inside a field".to_string());
    md.push("5. Do NOT repeat words already in your title, subtitle, or categories".to_string());
    md.push(String::new());
    md.join("\n")
}

fn render_pr_keywords(keywords: &[String]) -> String {
    let now = chrono::Utc::now().format("%B %-d, %Y %H:%M UTC").to_string();
    let mut md = vec![
        "# PR Competition Analyzer Keywords".to_string(),
        format!("Generated: {}", now),
        String::new(),
        "> These short phrases are for Publisher Rocket's Competition Analyzer.".to_string(),
        "> They are NOT the same as your KDP keyword strings.".to_string(),
        String::new(),
    ];
    for kw in keywords { md.push(format!("- `{}`", kw)); }
    md.push(String::new());
    md.join("\n")
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
