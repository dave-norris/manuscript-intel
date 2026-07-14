// analysis/genres.rs — Genre ranking and genre analysis (Phase 2)
//
// Scores a manuscript against the master genre list, produces structured
// genre data (industry classification, KDP paths, comps, signals), and
// renders reports from that data.

use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use super::{emit, err, extract_json_object, GenreResult, FolderRequest};
use crate::commands::call_llm;
use crate::db;

use super::chapters::{collect_chapters, phase1_summaries, build_combined_context};

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub(crate) struct AiGenreRank {
    pub genre:      String,
    pub confidence: u8,
    pub reason:     String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RankedGenre {
    pub genre:      String,
    pub confidence: u8,
    pub reason:     String,
    pub kdp_paths:  Vec<String>,
}

// ── Tauri commands ───────────────────────────────────────────────────────────

#[tauri::command]
pub async fn rank_genres_for_story(app: AppHandle, request: FolderRequest) -> GenreResult {
    let database = app.state::<db::Db>();

    let genre_data = {
        let conn = database.0.lock().unwrap();
        db::load_genre_data(&conn, &request.folder)
    };
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

    match ai_rank_genres(&request.provider, &request.api_key, &request.model, &description, &master_list).await {
        Err(e) => err(&e),
        Ok(ai_ranked) => {
            let mut ranked: Vec<RankedGenre> = ai_ranked.into_iter().map(|r| {
                let kdp_paths = crate::genre_taxonomy::kdp_paths_for_genre(&database, &r.genre, "Kindle")
                    .unwrap_or_default();
                RankedGenre { genre: r.genre, confidence: r.confidence, reason: r.reason, kdp_paths }
            }).collect();
            ranked.sort_by(|a, b| b.confidence.cmp(&a.confidence));

            for r in &ranked {
                emit(&app, &format!("  {}% \u{2014} {}{}", r.confidence, r.genre,
                    if r.kdp_paths.is_empty() { " (no mapped KDP path yet)".to_string() } else { String::new() }));
            }

            let now_disp = chrono::Utc::now().format("%B %-d, %Y %H:%M UTC").to_string();
            let mut lines = vec![
                "# Genre Ranking".to_string(),
                format!("Generated: {}", now_disp),
                String::new(),
                "Each genre is scored independently against the manuscript \u{2014} percentages do NOT sum to 100. A cross-genre book can score high on several genres at once; a lower score means a weaker but still real fit.".to_string(),
                String::new(),
            ];
            for r in &ranked {
                lines.push(format!("## {} \u{2014} {}%", r.genre, r.confidence));
                lines.push(String::new());
                lines.push(r.reason.clone());
                lines.push(String::new());
                if !r.kdp_paths.is_empty() {
                    lines.push("**Known KDP category path(s):**".to_string());
                    for p in &r.kdp_paths { lines.push(format!("- `{}`", p)); }
                } else {
                    lines.push("*No mapped KDP path yet for this genre. Run Category Finder to discover one \u{2014} it will be saved to the database automatically.*".to_string());
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
                emit(&app, &format!("  \u{26a0} Could not save ranking to database: {}", e));
            }
            let _ = db::save_document(&conn, &request.folder, "genre_ranking", &report);
            emit(&app, &format!("\u{2713} Ranking saved to database \u{2014} {} genre(s) ranked.", ranked.len()));

            GenreResult { success: true, report, error: String::new() }
        }
    }
}

#[tauri::command]
pub async fn analyze_genre(app: AppHandle, request: FolderRequest) -> GenreResult {
    let folder = PathBuf::from(&request.folder);
    if !folder.exists() { return err("Folder does not exist."); }

    let database = app.state::<db::Db>();
    let mut summaries = {
        let conn = database.0.lock().unwrap();
        db::load_chapter_summaries(&conn, &request.folder)
    };

    if summaries.is_empty() {
        emit(&app, "No summaries found \u{2014} running Phase 1 first...");
        let chapters = collect_chapters(&folder);
        if chapters.is_empty() { return err("No .md files found."); }
        phase1_summaries(&app, &database, &chapters, &request.folder, &request.provider, &request.api_key, &request.model).await;
        let conn = database.0.lock().unwrap();
        summaries = db::load_chapter_summaries(&conn, &request.folder);
    }

    if summaries.is_empty() { return err("Could not produce any chapter summaries."); }

    emit(&app, &format!("Phase 2: Analyzing {} chapter summaries...", summaries.len()));
    phase2_analyze(&app, &database, &request.folder, &summaries, &request.provider, &request.api_key, &request.model).await
}

// ── Phase 2 implementation ───────────────────────────────────────────────────

pub(crate) async fn phase2_analyze(
    app: &AppHandle,
    database: &db::Db,
    story_folder: &str,
    summaries: &[db::ChapterSummaryRow],
    provider: &str,
    api_key: &str,
    model: &str,
) -> GenreResult {
    let combined = build_combined_context(summaries);

    emit(app, &format!(
        "  Sending {} summaries ({} chars) to {}...",
        summaries.len(), combined.len(), model
    ));

    match call_ai_genre_analysis(provider, api_key, model, &combined).await {
        Err(e) => err(&format!("Phase 2 AI error: {}", e)),
        Ok(g) => {
            let conn = database.0.lock().unwrap();
            let _ = db::save_genre_data(
                &conn, story_folder,
                &g.industry_ebook, &g.industry_print, &g.genre_signals,
                &g.reader_demographic, &g.bookstore_shelving,
                &g.kdp_ebook, &g.kdp_print, &g.comps_ebook, &g.comps_print, &g.marketing_notes,
            );
            emit(app, "  \u{2713} Genre data saved to database.");
            let rendered = render_genre_analysis_md(&g);
            let _ = db::save_document(&conn, story_folder, "genre_analysis", &rendered);
            GenreResult { success: true, report: rendered, error: String::new() }
        }
    }
}

// ── AI calls ─────────────────────────────────────────────────────────────────

pub(crate) async fn ai_rank_genres(
    provider: &str,
    api_key: &str,
    model: &str,
    description: &str,
    master_list: &[db::GenreRow],
) -> Result<Vec<AiGenreRank>, String> {
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

    let raw = call_llm(provider, api_key, model, &system, description, 1200).await?;
    let clean = raw.trim()
        .trim_start_matches("```json").trim_start_matches("```")
        .trim_end_matches("```").trim();

    serde_json::from_str::<Vec<AiGenreRank>>(clean)
        .map_err(|e| format!("Parse error (genre ranking): {} | got: {}", e, &clean[..clean.len().min(300)]))
}

pub(crate) async fn call_ai_genre_analysis(
    provider: &str,
    api_key: &str,
    model: &str,
    combined: &str,
) -> Result<db::GenreDataRow, String> {
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

    let raw = call_llm(
        provider, api_key, model, system,
        &format!("Genre signals from all chapters:\n\n{}", combined),
        1500,
    ).await?;

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

// ── Rendering ────────────────────────────────────────────────────────────────

pub(crate) fn render_genre_analysis_md(g: &db::GenreDataRow) -> String {
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

pub(crate) fn render_full_report(g: &db::GenreDataRow, competition_done: bool) -> String {
    let json = serde_json::json!({
        "schema": "full_report_v1",
        "genre_analysis": serde_json::from_str::<serde_json::Value>(&render_genre_analysis_md(g)).unwrap_or_default(),
        "competition_done": competition_done,
    });
    json.to_string()
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Strip store-level prefixes from a KDP category path.
/// "Kindle Store > Kindle eBooks > Romance > Contemporary" -> "Romance > Contemporary"
/// "Books > Literature & Fiction > Women's Fiction" -> "Literature & Fiction > Women's Fiction"
pub(crate) fn strip_kdp_prefix(path: &str) -> String {
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

pub(crate) fn strip_kdp_paths(paths: Vec<String>) -> Vec<String> {
    paths.into_iter().map(|p| strip_kdp_prefix(&p)).collect()
}
