// analysis/pipeline.rs — Orchestration commands that compose the analysis pipeline.
//
// These commands call into chapters, genres, categories, keywords, and bisac
// to run multi-step analyses and assemble combined reports.

use std::path::PathBuf;
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

use super::{emit, err, extract_json_object, GenreResult, FolderRequest, AnalyzeStoryRequest};
use crate::commands::call_llm;
use crate::db;
use crate::models::KeywordResult;

use super::chapters::{collect_chapters, phase1_summaries};
use super::genres::{RankedGenre, ai_rank_genres, phase2_analyze, render_full_report};
use super::categories::{match_categories_by_store, rank_by_discoverability};
use super::bisac::ai_pick_bisac;
use super::keywords::{
    call_keyword_optimizer, call_keyword_optimizer_with_pool,
    derive_keyword_seeds, run_keyword_searches_canopy, run_keyword_searches_dataforseo,
    generate_discovery_keywords, render_kdp_keywords, render_search_terms,
};

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct AnalysisState {
    pub has_folder:                 bool,
    pub summary_count:              usize,
    pub has_genre_data:             bool,
    pub has_full_report:            bool,
    pub has_keywords:               bool,
    pub has_search_terms:           bool,
    pub has_competition:            bool,
    pub has_categories:             bool,
    pub has_genre_ranking:          bool,
    pub has_mapped_verified:        bool,
    pub has_bisac:                  bool,
    pub has_discovery_keywords:     bool,
    pub has_keyword_search_results: bool,
    pub has_zeigarnik:              bool,
    pub has_continuity_check:       bool,
    pub has_show_dont_tell:         bool,
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

// ── Analysis state check ──────────────────────────────────────────────────────

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
            has_search_terms:           !db::load_mi_search_terms(&conn, &folder).is_empty(),
            has_competition:            db::get_document(&conn, &folder, "competition_report").is_some(),
            has_categories:             db::has_category_results(&conn, &folder),
            has_genre_ranking:          db::has_genre_rankings(&conn, &folder),
            has_mapped_verified:        db::get_document(&conn, &folder, "mapped_categories").is_some(),
            has_bisac:                  db::has_bisac_classifications(&conn, &folder),
            has_discovery_keywords:     !db::load_discovery_keywords(&conn, &folder).is_empty(),
            has_keyword_search_results: db::has_keyword_search_results(&conn, &folder),
            has_zeigarnik:              db::has_zeigarnik_analysis(&conn, &folder),
            has_continuity_check:       db::get_document(&conn, &folder, "continuity_check").is_some(),
            has_show_dont_tell:         db::get_document(&conn, &folder, "show_dont_tell").is_some(),
        }
    }).await.unwrap()
}

// ── run_everything ────────────────────────────────────────────────────────────

/// Run everything except folder selection and chapter summaries:
/// Analyze Genre → Full Analysis → Optimize Keywords → Generate Search Terms
#[tauri::command]
pub async fn run_everything(app: AppHandle, request: FolderRequest) -> GenreResult {
    let folder = PathBuf::from(&request.folder);
    if !folder.exists() { return err("Folder does not exist."); }

    crate::reset_cancel();
    let database = app.state::<db::Db>();
    let run_ts = chrono::Utc::now().to_rfc3339();

    // ── Step 1: Ensure summaries exist ────────────────────────────────────
    let mut summaries = { let conn = database.0.lock().unwrap(); db::load_chapter_summaries(&conn, &request.folder) };
    if summaries.is_empty() {
        emit(&app, "Step 1: No summaries found — generating now...");
        let chapters = collect_chapters(&folder);
        if chapters.is_empty() { return err("No .md chapter files found."); }
        phase1_summaries(&app, &database, &chapters, &request.folder, &request.provider, &request.api_key, &request.model).await;
        let conn = database.0.lock().unwrap();
        summaries = db::load_chapter_summaries(&conn, &request.folder);
        if summaries.is_empty() { return err("Could not produce chapter summaries."); }
    } else {
        emit(&app, &format!("Step 1: {} summaries found — skipping.", summaries.len()));
    }
    if crate::is_cancelled() { return err("Cancelled."); }

    // ── Step 2: Genre analysis ─────────────────────────────────────────────
    emit(&app, "Step 2: Running genre analysis...");
    let genre_result = phase2_analyze(&app, &database, &request.folder, &summaries, &request.provider, &request.api_key, &request.model).await;
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
    { let conn = database.0.lock().unwrap(); let _ = db::save_document_at(&conn, &request.folder, "full_report", &full_report, &run_ts); }
    emit(&app, "  ✓ Full report saved to database.");
    if crate::is_cancelled() { return err("Cancelled."); }

    // ── Step 4: Optimize KDP keywords ─────────────────────────────────────
    emit(&app, "Step 4: Optimizing KDP keywords...");
    match call_keyword_optimizer(&request.provider, &request.api_key, &request.model, &genre_data, &genre_data.genre_signals).await {
        Ok((entries, strategy)) => {
            let conn = database.0.lock().unwrap();
            let _ = db::save_kdp_keywords(&conn, &request.folder, &entries, &strategy, "*(Generated from genre analysis.)*");
            let rendered = render_kdp_keywords(&entries, &strategy, "*(Generated from genre analysis.)*");
            let _ = db::save_document_at(&conn, &request.folder, "kdp_keywords", &rendered, &run_ts);
            emit(&app, "  ✓ KDP keywords saved to database.");
        }
        Err(e) => emit(&app, &format!("  ⚠ Keyword optimization failed: {}", e)),
    }
    if crate::is_cancelled() { return err("Cancelled."); }

    // ── Step 5: Generate search terms ──────────────────────────────────────
    emit(&app, "Step 5: Generating competition search terms...");
    let pr_system = r#"You are a book market research expert. Generate short search phrases for competition analysis.

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

    match call_llm(&request.provider, &request.api_key, &request.model, pr_system, &pr_user, 300).await {
        Ok(raw) => {
            if let Some(clean) = extract_json_object(&raw) {
                if let Ok(keywords) = serde_json::from_str::<Vec<String>>(&clean) {
                    let conn = database.0.lock().unwrap();
                    let _ = db::save_mi_search_terms(&conn, &request.folder, &keywords);
                    let rendered = render_search_terms(&keywords);
                    let _ = db::save_document_at(&conn, &request.folder, "mi_search_terms", &rendered, &run_ts);
                    emit(&app, &format!("  ✓ {} search terms saved to database.", keywords.len()));
                    for kw in &keywords { emit(&app, &format!("    • {}", kw)); }
                }
            } else {
                emit(&app, "  ⚠ Could not parse search terms response.");
            }
        }
        Err(e) => emit(&app, &format!("  ⚠ Search terms generation failed: {}", e)),
    }

    emit(&app, "✓ Analysis complete. Run Analyze Competition next.");

    GenreResult { success: true, report: full_report, error: String::new(), run_ts: run_ts.clone() }
}

// ── run_full_analysis ─────────────────────────────────────────────────────────

#[tauri::command]
pub async fn run_full_analysis(app: AppHandle, request: FolderRequest) -> GenreResult {
    let folder = PathBuf::from(&request.folder);
    if !folder.exists() { return err("Folder does not exist."); }

    let database = app.state::<db::Db>();
    let run_ts = chrono::Utc::now().to_rfc3339();

    // ── Phase 1 ──────────────────────────────────────────────────────────
    let mut summaries = { let conn = database.0.lock().unwrap(); db::load_chapter_summaries(&conn, &request.folder) };
    if summaries.is_empty() {
        emit(&app, "Phase 1: Generating chapter summaries...");
        let chapters = collect_chapters(&folder);
        if chapters.is_empty() { return err("No .md files found."); }
        phase1_summaries(&app, &database, &chapters, &request.folder, &request.provider, &request.api_key, &request.model).await;
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
        let r = phase2_analyze(&app, &database, &request.folder, &summaries, &request.provider, &request.api_key, &request.model).await;
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
    { let conn = database.0.lock().unwrap(); let _ = db::save_document_at(&conn, &request.folder, "full_report", &full_report, &run_ts); }
    emit(&app, "✓ Full report saved to database.");

    GenreResult { success: true, report: full_report, error: String::new(), run_ts: run_ts.clone() }
}

// ── find_genres_and_categories_for_story ──────────────────────────────────────

#[tauri::command]
pub async fn find_genres_and_categories_for_story(app: AppHandle, request: FolderRequest) -> GenreResult {
    let cancel = crate::cancel_notify();
    tokio::select! {
        result = find_genres_and_categories_inner(app, request) => result,
        _ = cancel.notified() => err("Cancelled."),
    }
}

async fn find_genres_and_categories_inner(app: AppHandle, request: FolderRequest) -> GenreResult {
    let database = app.state::<db::Db>();
    let run_ts = chrono::Utc::now().to_rfc3339();

    // ── Ensure genre_data exists ──
    let mut genre_data = { let conn = database.0.lock().unwrap(); db::load_genre_data(&conn, &request.folder) };
    if genre_data.is_none() {
        emit(&app, "No genre data yet — running Analyze first...");
        let folder_path = PathBuf::from(&request.folder);
        if !folder_path.exists() { return err("Folder does not exist."); }

        let mut summaries = { let conn = database.0.lock().unwrap(); db::load_chapter_summaries(&conn, &request.folder) };
        if summaries.is_empty() {
            let chapters = collect_chapters(&folder_path);
            if chapters.is_empty() { return err("No .md chapter files found."); }
            phase1_summaries(&app, &database, &chapters, &request.folder, &request.provider, &request.api_key, &request.model).await;
            let conn = database.0.lock().unwrap();
            summaries = db::load_chapter_summaries(&conn, &request.folder);
        }
        if summaries.is_empty() { return err("Could not produce chapter summaries."); }

        let r = phase2_analyze(&app, &database, &request.folder, &summaries, &request.provider, &request.api_key, &request.model).await;
        if !r.success { return err(&r.error); }
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
        let master_list = crate::genre_taxonomy::master_genre_list(&database)
            .map_err(|e| format!("Could not load genre list from database: {}", e));
        let master_list = match master_list {
            Ok(l) => l,
            Err(e) => return err(&e),
        };

        let description = format!(
            "{}\n\nKDP paths already identified: {}\n\n{}",
            genre_data.industry_ebook, genre_data.kdp_ebook.join("; "), genre_data.genre_signals
        );

        let ai_ranked = match ai_rank_genres(&request.provider, &request.api_key, &request.model, &description, &master_list).await {
            Ok(r) => r,
            Err(e) => return err(&format!("Genre ranking failed: {}", e)),
        };

        let mut ranked: Vec<RankedGenre> = ai_ranked.into_iter().map(|r| {
            let kdp_paths = crate::genre_taxonomy::kdp_paths_for_genre(&database, &r.genre, "Kindle").unwrap_or_default();
            RankedGenre { genre: r.genre, confidence: r.confidence, reason: r.reason, kdp_paths }
        }).collect();
        ranked.sort_by(|a, b| b.confidence.cmp(&a.confidence));

        let conn = database.0.lock().unwrap();
        let rows: Vec<(String, u8, String)> = ranked.iter().map(|r| (r.genre.clone(), r.confidence, r.reason.clone())).collect();
        let _ = db::replace_genre_rankings(&conn, &request.folder, &rows);
        let genre_ranking_md = {
            let mut s = vec!["# Genre Ranking".to_string(), String::new()];
            for r in &ranked { s.push(format!("## {} — {}%", r.genre, r.confidence)); s.push(String::new()); s.push(r.reason.clone()); s.push(String::new()); }
            s.join("\n")
        };
        let _ = db::save_document_at(&conn, &request.folder, "genre_ranking", &genre_ranking_md, &run_ts);

        ranked
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

    // ── KDP Categories, both formats ──
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

        let result = match_categories_by_store(&app, &database, &request.folder, store, &base_description, &genre_terms, &request.provider, &request.api_key, &request.model).await;

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
                if !q.top_books.is_empty() {
                    kdp_section.push(String::new());
                    kdp_section.push("   **Current Top Sellers:**".to_string());
                    for (rank, book) in q.top_books.iter().enumerate() {
                        let amazon_link = format!("https://www.amazon.com/dp/{}", book.asin);
                        let img_tag = book.image_url.as_deref()
                            .map(|url| format!("   <img src=\"{}\" height=\"60\" /> ", url))
                            .unwrap_or_default();
                        kdp_section.push(format!("   {}{}. [{}]({})", img_tag, rank + 1, book.title, amazon_link));
                    }
                    kdp_section.push(String::new());
                }
            }
        }
        kdp_section.push(String::new());
    }
    report_sections.push(kdp_section.join("\n"));

    // ── BISAC, ebook then print if different ───────────────────────
    emit(&app, "Classifying BISAC subject headings...");
    let bisac_master = { let conn = database.0.lock().unwrap(); db::master_bisac_list(&conn) };
    let same_as_ebook = genre_data.industry_print.trim().eq_ignore_ascii_case(genre_data.industry_ebook.trim());

    let ebook_desc = format!("{}\n\n{}", genre_data.industry_ebook, genre_data.genre_signals);
    let ebook_picks = ai_pick_bisac(&request.provider, &request.api_key, &request.model, &ebook_desc, &bisac_master).await.unwrap_or_default();
    {
        let conn = database.0.lock().unwrap();
        let rows: Vec<(String, String, u8, String)> = ebook_picks.iter().map(|(c, h, cf, r)| (c.clone(), h.clone(), *cf, r.clone())).collect();
        let _ = db::replace_bisac_classifications(&conn, &request.folder, "ebook", &rows);
    }

    let print_picks_opt = if same_as_ebook {
        let conn = database.0.lock().unwrap();
        let rows: Vec<(String, String, u8, String)> = ebook_picks.iter().map(|(c, h, cf, r)| (c.clone(), h.clone(), *cf, r.clone())).collect();
        let _ = db::replace_bisac_classifications(&conn, &request.folder, "print", &rows);
        None
    } else {
        let print_desc = format!("{}\n\n{}", genre_data.industry_print, genre_data.genre_signals);
        let print_picks = ai_pick_bisac(&request.provider, &request.api_key, &request.model, &print_desc, &bisac_master).await.unwrap_or_default();
        let conn = database.0.lock().unwrap();
        let rows: Vec<(String, String, u8, String)> = print_picks.iter().map(|(c, h, cf, r)| (c.clone(), h.clone(), *cf, r.clone())).collect();
        let _ = db::replace_bisac_classifications(&conn, &request.folder, "print", &rows);
        Some(print_picks)
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

    // ── Positioning context ──
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
        "Full pipeline in one pass: genre ranking, KDP categories (Kindle eBook + Paperback, verified live via Canopy API), BISAC classification (ebook + print), and positioning context.".to_string(),
        String::new(), "---".to_string(), String::new(),
    ];
    lines.push(report_sections.join("\n---\n\n"));
    let report = lines.join("\n");

    { let conn = database.0.lock().unwrap(); let _ = db::save_document_at(&conn, &request.folder, "genres_and_categories", &report, &run_ts); }
    emit(&app, "✓ Genres & Categories report saved to database.");

    GenreResult { success: true, report, error: String::new(), run_ts: run_ts.clone() }
}

// ── Combined Report Assembly ──────────────────────────────────────────────────

/// Assembles all pipeline output sections into a single structured JSON document.
pub(crate) fn render_combined_report(
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

// ── analyze_story ─────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn analyze_story(app: AppHandle, request: AnalyzeStoryRequest) -> GenreResult {
    let cancel = crate::cancel_notify();
    tokio::select! {
        result = analyze_story_inner(app, request) => result,
        _ = cancel.notified() => err("Cancelled."),
    }
}

async fn analyze_story_inner(app: AppHandle, request: AnalyzeStoryRequest) -> GenreResult {
    if request.api_key.is_empty() { return err("No API key set. Go to Settings."); }
    if request.model.is_empty() { return err("No model selected. Go to Settings."); }

    let database = app.state::<db::Db>();
    let run_ts = if request.run_time.is_empty() { chrono::Utc::now().to_rfc3339() } else { request.run_time.clone() };

    // ── Step 1: Summaries ──────────────────────────────────────────────────
    emit(&app, "Step 1: Chapter summaries...");
    {
        let folder_path = PathBuf::from(&request.folder);
        if !folder_path.exists() { return err("Folder does not exist."); }

        crate::reset_cancel();

        if request.force_resummarize {
            emit(&app, "  Force re-summarize — deleting existing summaries...");
            let conn = database.0.lock().unwrap();
            let _ = db::delete_chapter_summaries(&conn, &request.folder);
        }

        let chapters = collect_chapters(&folder_path);
        if chapters.is_empty() { return err("No .md chapter files found."); }

        let (done, skipped) = phase1_summaries(&app, &database, &chapters, &request.folder, &request.provider, &request.api_key, &request.model).await;
        emit(&app, &format!("  ✓ {} summarized, {} skipped.", done, skipped));
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
            let _ = db::save_document_at(&conn, &request.folder, "chapter_summaries", &cs_json, &run_ts);
        }
    }
    if crate::is_cancelled() { return err("Cancelled."); }

    // ── Step 2: Genre Analysis ─────────────────────────────────────────────
    emit(&app, "Step 2: Genre analysis...");
    let genre_data_existing = { let conn = database.0.lock().unwrap(); db::load_genre_data(&conn, &request.folder) };
    if genre_data_existing.is_none() {
        let summaries = { let conn = database.0.lock().unwrap(); db::load_chapter_summaries(&conn, &request.folder) };
        if summaries.is_empty() { return err("No chapter summaries available."); }
        let r = phase2_analyze(&app, &database, &request.folder, &summaries, &request.provider, &request.api_key, &request.model).await;
        if !r.success { return err(&r.error); }
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
        let master_list = crate::genre_taxonomy::master_genre_list(&database)
            .map_err(|e| format!("Could not load genre list from database: {}", e));
        let master_list = match master_list {
            Ok(l) => l,
            Err(e) => return err(&e),
        };

        let description = format!(
            "{}\n\nKDP paths already identified: {}\n\n{}",
            genre_data.industry_ebook, genre_data.kdp_ebook.join("; "), genre_data.genre_signals
        );

        let ai_ranked = match ai_rank_genres(&request.provider, &request.api_key, &request.model, &description, &master_list).await {
            Ok(r) => r,
            Err(e) => return err(&format!("Genre ranking failed: {}", e)),
        };

        let mut ranked: Vec<RankedGenre> = ai_ranked.into_iter().map(|r| {
            let kdp_paths = crate::genre_taxonomy::kdp_paths_for_genre(&database, &r.genre, "Kindle").unwrap_or_default();
            RankedGenre { genre: r.genre, confidence: r.confidence, reason: r.reason, kdp_paths }
        }).collect();
        ranked.sort_by(|a, b| b.confidence.cmp(&a.confidence));

        let conn = database.0.lock().unwrap();
        let rows: Vec<(String, u8, String)> = ranked.iter().map(|r| (r.genre.clone(), r.confidence, r.reason.clone())).collect();
        let _ = db::replace_genre_rankings(&conn, &request.folder, &rows);

        // Save genre ranking as a standalone report
        let ranking_json = serde_json::json!({
            "schema": "genre_ranking_v1",
            "genres": ranked.iter().map(|r| serde_json::json!({
                "genre": r.genre, "confidence": r.confidence, "reason": r.reason,
            })).collect::<Vec<_>>(),
        }).to_string();
        let _ = db::save_document_at(&conn, &request.folder, "genre_ranking", &ranking_json, &run_ts);

        ranked
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

    let is_wide = request.platform == "wide";

    // ── Step 4: KDP Categories (both stores) ───────────────────────────────
    let mut kindle_top_categories: Vec<String> = Vec::new();
    let mut print_top_categories: Vec<String> = Vec::new();
    let kdp_categories_section: String;
    let mut kdp_stores_json: Vec<serde_json::Value> = Vec::new();

    if is_wide {
        emit(&app, "Step 4: Skipping KDP categories (Wide distribution mode).");
        kdp_categories_section = serde_json::json!({ "stores": [] }).to_string();
    } else {
    emit(&app, "Step 4: Matching KDP categories...");
    let base_description = format!("{}\n\n{}", genre_data.industry_ebook, genre_data.genre_signals);

    for (store, label, top_cats) in [
        ("Kindle", "Kindle eBook", &mut kindle_top_categories as &mut Vec<String>),
        ("Books", "Paperback", &mut print_top_categories as &mut Vec<String>),
    ] {
        let total_catalog = { let conn = database.0.lock().unwrap(); db::kdp_category_count(&conn, store) };
        if total_catalog < 50 {
            kdp_stores_json.push(serde_json::json!({ "store": label, "error": "Catalog nearly empty — import WinningCat data." }));
            continue;
        }

        let result = match_categories_by_store(&app, &database, &request.folder, store, &base_description, &genre_terms, &request.provider, &request.api_key, &request.model).await;

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
                "top_books": q.top_books.iter().map(|b| serde_json::json!({
                    "title": b.title,
                    "asin": b.asin,
                    "image_url": b.image_url,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        }));

        if crate::is_cancelled() { return err("Cancelled."); }
    }
    kdp_categories_section = serde_json::json!({ "stores": kdp_stores_json }).to_string();
    } // end of if !is_wide for KDP categories
    if crate::is_cancelled() { return err("Cancelled."); }

    // ── Step 5: Generate search terms (KDP only) ───────────────────────────
    if !is_wide {
    emit(&app, "Step 5: Generating competition search terms...");
    {
        let system = r#"You are a book market research expert. Generate short search phrases for competition analysis.

Rules:
- 2-4 words maximum per phrase
- Plain English, no special characters
- Think like a reader browsing Amazon
- Include: genre combinations, setting descriptors, theme words, reader mood phrases

Return ONLY a JSON array of strings. No markdown, no preamble."#;

        let user = format!(
            "Book genre: {}\nKDP categories: {}\nGenre signals:\n{}",
            genre_data.industry_ebook,
            genre_data.kdp_ebook.iter()
                .map(|p| p.split('>').last().unwrap_or(p).trim().to_string())
                .collect::<Vec<_>>().join(", "),
            &genre_data.genre_signals[..genre_data.genre_signals.len().min(500)]
        );

        match call_llm(&request.provider, &request.api_key, &request.model, system, &user, 300).await {
            Ok(raw) => {
                if let Some(clean) = extract_json_object(&raw) {
                    if let Ok(keywords) = serde_json::from_str::<Vec<String>>(&clean) {
                        let conn = database.0.lock().unwrap();
                        let _ = db::save_mi_search_terms(&conn, &request.folder, &keywords);
                        let rendered = render_search_terms(&keywords);
                        let _ = db::save_document_at(&conn, &request.folder, "mi_search_terms", &rendered, &run_ts);
                        emit(&app, &format!("  ✓ {} search terms saved.", keywords.len()));
                    }
                } else {
                    emit(&app, "  ⚠ Could not parse search terms response.");
                }
            }
            Err(e) => emit(&app, &format!("  ⚠ Search terms generation failed: {}", e)),
        }
    }
    } // end of if !is_wide for search terms
    if crate::is_cancelled() { return err("Cancelled."); }

    // ── Step 6: BISAC Classification ───────────────────────────────────────
    emit(&app, "Step 6: BISAC classification...");
    let bisac_section: String = {
        let bisac_master = { let conn = database.0.lock().unwrap(); db::master_bisac_list(&conn) };
        let same_as_ebook = genre_data.industry_print.trim().eq_ignore_ascii_case(genre_data.industry_ebook.trim());

        let ebook_desc = format!("{}\n\n{}", genre_data.industry_ebook, genre_data.genre_signals);
        let ebook_picks = ai_pick_bisac(&request.provider, &request.api_key, &request.model, &ebook_desc, &bisac_master).await.unwrap_or_default();
        {
            let conn = database.0.lock().unwrap();
            let rows: Vec<(String, String, u8, String)> = ebook_picks.iter().map(|(c, h, cf, r)| (c.clone(), h.clone(), *cf, r.clone())).collect();
            let _ = db::replace_bisac_classifications(&conn, &request.folder, "ebook", &rows);
        }

        let print_picks = if same_as_ebook {
            let conn = database.0.lock().unwrap();
            let rows: Vec<(String, String, u8, String)> = ebook_picks.iter().map(|(c, h, cf, r)| (c.clone(), h.clone(), *cf, r.clone())).collect();
            let _ = db::replace_bisac_classifications(&conn, &request.folder, "print", &rows);
            None
        } else {
            let print_desc = format!("{}\n\n{}", genre_data.industry_print, genre_data.genre_signals);
            let picks = ai_pick_bisac(&request.provider, &request.api_key, &request.model, &print_desc, &bisac_master).await.unwrap_or_default();
            let conn = database.0.lock().unwrap();
            let rows: Vec<(String, String, u8, String)> = picks.iter().map(|(c, h, cf, r)| (c.clone(), h.clone(), *cf, r.clone())).collect();
            let _ = db::replace_bisac_classifications(&conn, &request.folder, "print", &rows);
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
    };
    // Save BISAC as standalone report
    { let conn = database.0.lock().unwrap(); let _ = db::save_document_at(&conn, &request.folder, "bisac_classification", &bisac_section, &run_ts); }
    if crate::is_cancelled() { return err("Cancelled."); }

    // ── Step 7: Keyword Search (KDP only) ────────────────────────────────────
    let keyword_pool: Vec<KeywordResult> = if is_wide {
        emit(&app, "Step 7: Skipping keyword search (Wide distribution mode).");
        Vec::new()
    } else {
    emit(&app, "Step 7: Keyword search...");
    let top_cats_for_seeds: Vec<String> = kindle_top_categories.iter().take(2).cloned().collect();
        let seeds = derive_keyword_seeds(&genre_data.industry_ebook, &top_cats_for_seeds);
        if seeds.is_empty() {
            emit(&app, "  ⚠ No seeds derived — skipping keyword search.");
            Vec::new()
        } else {
            emit(&app, &format!("  Seeds: {:?}", seeds));
            if !request.dataforseo_login.is_empty() && !request.dataforseo_password.is_empty() {
                run_keyword_searches_dataforseo(&app, &request.folder, &seeds, &request.dataforseo_login, &request.dataforseo_password).await
            } else if !request.canopy_api_key.is_empty() {
                emit(&app, "⚠ DataForSEO credentials not set — falling back to Canopy for keyword search. Add DataForSEO login/password in Settings for real Amazon search volume data.");
                run_keyword_searches_canopy(&app, &request.folder, &seeds, &request.canopy_api_key).await
            } else {
                emit(&app, "  ⚠ No DataForSEO or Canopy credentials — skipping keyword search.");
                Vec::new()
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
        let _ = db::save_document_at(&conn, &request.folder, "keyword_search", &ks_json, &run_ts);
    }
    if crate::is_cancelled() { return err("Cancelled."); }

    // ── Step 8: KDP Keywords (KDP only) ────────────────────────────────────
    let (kdp_keyword_entries, kdp_keyword_strategy) = if is_wide {
        emit(&app, "Step 8: Skipping KDP keywords (Wide distribution mode).");
        (Vec::new(), String::new())
    } else {
    emit(&app, "Step 8: Optimizing KDP keywords...");
    {
        let res = call_keyword_optimizer_with_pool(&request.provider, &request.api_key, &request.model, &genre_data, &genre_data.genre_signals, &keyword_pool).await;

        match res {
            Ok((entries, strategy)) => {
                let source_note = if keyword_pool.is_empty() {
                    "*(Generated from genre analysis — no keyword search data available.)*"
                } else {
                    "*(Enhanced with real Amazon search volume data.)*"
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
    }
    }; // end kdp_keyword_entries
    if crate::is_cancelled() { return err("Cancelled."); }

    // ── Step 9: Discovery Keywords ─────────────────────────────────────────
    emit(&app, "Step 9: Generating discovery keywords...");
    let discovery_entries: Vec<db::DiscoveryKeywordEntry> = {
        let res = generate_discovery_keywords(&request.provider, &request.api_key, &request.model, &genre_data).await;

        match res {
            Ok(entries) => {
                // Enrich with Google search volume from DataForSEO if credentials available
                let enriched = if !request.dataforseo_login.is_empty() && !request.dataforseo_password.is_empty() && !entries.is_empty() {
                    emit(&app, "  Enriching with Google search volume via DataForSEO...");
                    let phrases: Vec<String> = entries.iter().map(|e| e.phrase.clone()).collect();
                    let client = crate::dataforseo::DataForSeoClient::new(&request.dataforseo_login, &request.dataforseo_password);
                    match client {
                        Ok(c) => match c.google_search_volume(&phrases).await {
                            Ok(volumes) => {
                                entries.into_iter().map(|mut e| {
                                    if let Some(v) = volumes.iter().find(|v| v.keyword.to_lowercase() == e.phrase.to_lowercase()) {
                                        e.rationale = format!("{}/mo Google — {}", v.search_volume, e.rationale);
                                    }
                                    e
                                }).collect()
                            }
                            Err(err) => { emit(&app, &format!("  ⚠ DataForSEO volume lookup failed: {}", err)); entries }
                        }
                        Err(err) => { emit(&app, &format!("  ⚠ DataForSEO client error: {}", err)); entries }
                    }
                } else {
                    entries
                };

                let conn = database.0.lock().unwrap();
                let _ = db::save_discovery_keywords(&conn, &request.folder, &enriched);
                // Save as standalone report
                let dk_json = serde_json::json!({
                    "schema": "discovery_keywords_v1",
                    "keywords": enriched.iter().map(|e| serde_json::json!({ "phrase": e.phrase, "rationale": e.rationale })).collect::<Vec<_>>(),
                }).to_string();
                let _ = db::save_document_at(&conn, &request.folder, "discovery_keywords", &dk_json, &run_ts);
                emit(&app, &format!("  ✓ {} discovery keywords saved.", enriched.len()));
                enriched
            }
            Err(e) => {
                emit(&app, &format!("  ⚠ Discovery keywords failed: {} — continuing.", e));
                Vec::new()
            }
        }
    };
    if crate::is_cancelled() { return err("Cancelled."); }

    // ── Step 10: Assemble Combined Report ───────────────────────────────────
    emit(&app, "Step 10: Assembling combined report...");

    // KDP paste section
    let kdp_paste = render_kdp_paste_section(&kindle_top_categories, &print_top_categories, &kdp_keyword_entries);

    // KDP keywords section
    let source_note = if keyword_pool.is_empty() {
        "*(Generated from genre analysis — no keyword search data available.)*"
    } else {
        "*(Enhanced with real Amazon search volume data.)*"
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

    { let conn = database.0.lock().unwrap(); let _ = db::save_document_at(&conn, &request.folder, "analysis", &report, &run_ts); }
    emit(&app, "✓ Full analysis report saved.");

    GenreResult { success: true, report, error: String::new(), run_ts: run_ts.clone() }
}

// ── KDP Paste Section Renderer ─────────────────────────────────────────────────

/// Renders the "KDP Metadata — Ready to Paste" section that mirrors the KDP
/// website's actual input layout.
pub(crate) fn render_kdp_paste_section(
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

// ── Craft pipeline ────────────────────────────────────────────────────────────

/// Request for the craft analysis pipeline.
/// The frontend sends which reports to run; this command handles ordering and execution.
#[derive(serde::Deserialize)]
pub struct CraftPipelineRequest {
    pub folder:           String,
    pub selected:         Vec<String>,
    pub provider:         String,
    pub api_key:          String,
    pub model:            String,           // default fallback
    #[serde(default)]
    pub model_summaries:  String,           // override for chapter summaries
    #[serde(default)]
    pub model_continuity: String,           // override for continuity check
    #[serde(default)]
    pub model_sdt:        String,           // override for show don't tell
    /// "manuscript" or "series"
    #[serde(default)]
    pub continuity_scope: String,
    /// Only used when continuity_scope == "series"
    #[serde(default)]
    pub series_id:        i64,
}

/// Runs the selected craft-platform reports in the correct order.
/// Chapter summaries → Zeigarnik → Continuity. Each is optional based on `selected`.
#[tauri::command]
pub async fn run_craft_pipeline(app: AppHandle, request: CraftPipelineRequest) -> GenreResult {
    let cancel = crate::cancel_notify();
    tokio::select! {
        result = run_craft_pipeline_inner(app, request) => result,
        _ = cancel.notified() => err("Cancelled."),
    }
}

async fn run_craft_pipeline_inner(app: AppHandle, request: CraftPipelineRequest) -> GenreResult {
    let folder = PathBuf::from(&request.folder);
    if !folder.exists() { return err("Folder does not exist."); }

    // Resolve per-function models (fall back to default)
    let model_summaries = if request.model_summaries.is_empty() { &request.model } else { &request.model_summaries };
    let model_continuity = if request.model_continuity.is_empty() { &request.model } else { &request.model_continuity };
    let model_sdt = if request.model_sdt.is_empty() { &request.model } else { &request.model_sdt };

    crate::reset_cancel();
    let database = app.state::<db::Db>();
    let run_ts = chrono::Utc::now().to_rfc3339();
    let needs_ai = request.selected.iter().any(|s| s == "chapter_summaries" || s == "continuity_check");

    if needs_ai && (request.api_key.is_empty() || request.model.is_empty()) {
        return err("An API key and model are required for the selected reports. Set them in Settings.");
    }

    // ── Chapter Summaries ─────────────────────────────────────────────────
    if request.selected.contains(&"chapter_summaries".to_string()) {
        emit(&app, &format!("Generating chapter summaries... [{}: {}]", request.provider, model_summaries));
        let chapters = collect_chapters(&folder);
        if chapters.is_empty() { return err("No .md chapter files found."); }

        let (done, skipped) = phase1_summaries(
            &app, &database, &chapters, &request.folder,
            &request.provider, &request.api_key, model_summaries,
        ).await;
        emit(&app, &format!("✓ Chapter summaries complete ({} new, {} skipped).", done, skipped));

        // Save as report
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
            let _ = db::save_document_at(&conn, &request.folder, "chapter_summaries", &cs_json, &run_ts);
        }

        if crate::is_cancelled() { return err("Cancelled."); }
    }

    // ── Zeigarnik Effect ──────────────────────────────────────────────────
    if request.selected.contains(&"zeigarnik_analysis".to_string()) {
        emit(&app, "Running Zeigarnik effect analysis (algorithmic — no AI)...");
        let zr = super::zeigarnik::analyze_zeigarnik_for_story(
            app.clone(),
            super::zeigarnik::ZeigarnikRequest { folder: request.folder.clone() },
        ).await;
        if zr.success {
            emit(&app, "✓ Zeigarnik analysis complete.");
        } else {
            emit(&app, &format!("✗ Zeigarnik: {}", zr.error));
            return zr;
        }
        if crate::is_cancelled() { return err("Cancelled."); }
    }

    // ── Continuity Check ──────────────────────────────────────────────────
    if request.selected.contains(&"continuity_check".to_string()) {
        if request.continuity_scope == "series" && request.series_id > 0 {
            emit(&app, &format!("Running continuity check across the series... [{}: {}]", request.provider, model_continuity));
            let cr = super::continuity::check_continuity_for_series(
                app.clone(),
                super::continuity::SeriesContinuityRequest {
                    series_id: request.series_id,
                    provider: request.provider.clone(),
                    api_key: request.api_key.clone(),
                    model: model_continuity.clone(),
                },
            ).await;
            if cr.success {
                emit(&app, "✓ Series continuity check complete.");
            } else {
                emit(&app, &format!("✗ Continuity: {}", cr.error));
                return cr;
            }
        } else {
            emit(&app, &format!("Running continuity check for this manuscript... [{}: {}]", request.provider, model_continuity));
            let cr = super::continuity::check_continuity_for_story(
                app.clone(),
                super::continuity::ContinuityRequest {
                    folder: request.folder.clone(),
                    provider: request.provider.clone(),
                    api_key: request.api_key.clone(),
                    model: model_continuity.clone(),
                },
            ).await;
            if cr.success {
                emit(&app, "✓ Continuity check complete.");
            } else {
                emit(&app, &format!("✗ Continuity: {}", cr.error));
                return cr;
            }
        }
        if crate::is_cancelled() { return err("Cancelled."); }
    }

    // ── Show Don't Tell ───────────────────────────────────────────────────
    if request.selected.contains(&"show_dont_tell".to_string()) {
        let sdt = super::show_dont_tell::check_show_dont_tell(
            app.clone(),
            super::show_dont_tell::ShowDontTellRequest {
                folder: request.folder.clone(),
                provider: request.provider.clone(),
                api_key: request.api_key.clone(),
                model: model_sdt.clone(),
            },
        ).await;
        if !sdt.success {
            emit(&app, &format!("✗ Show Don't Tell: {}", sdt.error));
            return sdt;
        }
        if crate::is_cancelled() { return err("Cancelled."); }
    }

    emit(&app, "✓ Done.");
    GenreResult { success: true, report: String::new(), error: String::new(), run_ts }
}
