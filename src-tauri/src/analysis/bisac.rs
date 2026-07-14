// analysis/bisac.rs — BISAC subject-code classification
//
// BISAC is the industry-standard subject code system (maintained by BISG)
// submitted as metadata for KDP Print and any wide/Ingram distribution.
// Convention is max 3 codes per book, primary first.

use serde::Deserialize;
use tauri::{AppHandle, Manager};

use super::{emit, err, GenreResult, FolderRequest};
use crate::commands::call_llm;
use crate::db;

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct AiBisacPick { code: String, confidence: u8, reason: String }

// ── Tauri command ────────────────────────────────────────────────────────────

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
                    "**On discoverability:** unlike KDP categories, there is no live data source for BISAC — no tool covers it, and Amazon has no browse mechanism for it. When two codes are close in fit, a more specific heading is preferred over a generic \"/ General\" one as a structural best-practice, not measured data.".to_string(),
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

// ── Core logic ───────────────────────────────────────────────────────────────

pub(crate) fn ai_pick_bisac(provider: &str, api_key: &str, model: &str, description: &str, master_list: &[db::BisacCodeRow])
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

/// When two codes are close in fit confidence (within 5 points), prefer a more
/// specific heading over a catch-all "/ General" one. Only re-orders genuinely
/// close calls — a clear fit winner from the AI is never overridden.
pub(crate) fn resort_bisac_for_specificity(mut picks: Vec<(String, String, u8, String)>) -> Vec<(String, String, u8, String)> {
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
