// analysis/show_dont_tell.rs — AI-assisted "Show Don't Tell" checker.
//
// Sends each chapter to the LLM asking it to identify passages where the
// author *tells* the reader something (emotions, reactions, judgments) rather
// than *showing* through action, dialogue, or sensory detail.
//
// The report includes the offending text plus surrounding context so the
// author can see exactly where the problem is.

use std::path::PathBuf;
use tauri::{AppHandle, Manager};

use super::{emit, err, GenreResult};
use super::chapters::{collect_chapters, extract_title};
use crate::db;

// ── Request ──────────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct ShowDontTellRequest {
    pub folder:   String,
    pub provider: String,
    pub api_key:  String,
    pub model:    String,
    #[serde(default)]
    pub bible_path: String,
}

// ── AI response shape ────────────────────────────────────────────────────────

#[derive(serde::Deserialize, Clone, Debug)]
struct AiViolation {
    #[serde(default)]
    telling_text: String,
    #[serde(default)]
    context:      String,
    #[serde(default)]
    why:          String,
    #[serde(default)]
    severity:     String,  // "minor" | "moderate" | "major"
}

// ── Command ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn check_show_dont_tell(app: AppHandle, request: ShowDontTellRequest) -> GenreResult {
    let cancel = crate::cancel_notify();
    tokio::select! {
        result = check_inner(app, request) => result,
        _ = cancel.notified() => err("Cancelled."),
    }
}

async fn check_inner(app: AppHandle, request: ShowDontTellRequest) -> GenreResult {
    let folder = PathBuf::from(&request.folder);
    if !folder.exists() { return err("Folder does not exist."); }
    if request.api_key.is_empty() || request.model.is_empty() {
        return err("Show Don't Tell requires an API key and model. Set them in Settings.");
    }

    crate::reset_cancel();
    let database = app.state::<db::Db>();
    let run_ts = chrono::Utc::now().to_rfc3339();

    let chapters = collect_chapters(&folder);
    if chapters.is_empty() { return err("No .md chapter files found."); }

    let bible = crate::prompts::load_bible_for_story(&request.folder, &request.bible_path);

    emit(&app, &format!("Checking {} chapter(s) for show-don't-tell violations...", chapters.len()));

    let mut all_findings: Vec<serde_json::Value> = Vec::new();
    let mut total_violations = 0usize;

    for (i, path) in chapters.iter().enumerate() {
        if crate::is_cancelled() { return err("Cancelled."); }

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let filename = path.strip_prefix(&folder)
            .unwrap_or(path)
            .to_string_lossy().to_string();
        let title = extract_title(&content).unwrap_or_else(|| filename.clone());

        // Use preprocessed text (cached)
        let processed = {
            let conn = database.0.lock().unwrap();
            crate::prompts::get_preprocessed(&conn, &request.folder, &filename, "sdt_check", path)
                .unwrap_or_else(|| {
                    let p = crate::prompts::preprocess_for_sdt(&content);
                    crate::prompts::store_preprocessed(&conn, &request.folder, &filename, "sdt_check", &p, path);
                    p
                })
        };

        emit(&app, &format!("[{}/{}] {} — checking...", i + 1, chapters.len(), filename));

        let violations = match extract_violations(
            &database, &request.provider, &request.api_key, &request.model,
            &filename, &processed, &bible,
        ).await {
            Ok(v) => v,
            Err(e) => {
                emit(&app, &format!("  ⚠ {}: {}", filename, e));
                continue;
            }
        };

        if violations.is_empty() {
            emit(&app, &format!("  ✓ {} — clean", filename));
        } else {
            emit(&app, &format!("  → {} — {} violation(s)", filename, violations.len()));
            total_violations += violations.len();

            all_findings.push(serde_json::json!({
                "file": filename,
                "title": title,
                "chapter_index": i,
                "violations": violations.iter().map(|v| serde_json::json!({
                    "telling_text": v.telling_text,
                    "context": v.context,
                    "why": v.why,
                    "severity": v.severity,
                })).collect::<Vec<_>>(),
            }));
        }
    }

    emit(&app, &format!("✓ Show Don't Tell complete — {} violation(s) across {} chapter(s).",
        total_violations, all_findings.len()));

    // Build and save report
    let report = serde_json::json!({
        "schema": "show_dont_tell_v1",
        "note": "AI-assisted: the model identifies passages that tell instead of show. Severity is subjective — use as a prompt to revisit, not a verdict.",
        "summary": {
            "chapters_checked": chapters.len(),
            "chapters_with_violations": all_findings.len(),
            "total_violations": total_violations,
        },
        "chapters": all_findings,
    }).to_string();

    {
        let conn = database.0.lock().unwrap();
        let _ = db::save_document_at(&conn, &request.folder, "show_dont_tell", &report, &run_ts);
    }

    GenreResult { success: true, report: String::new(), error: String::new(), run_ts }
}

// ── AI extraction ────────────────────────────────────────────────────────────

async fn extract_violations(
    db: &db::Db,
    provider: &str, api_key: &str, model: &str,
    filename: &str, content: &str, bible: &str,
) -> Result<Vec<AiViolation>, String> {
    use std::collections::HashMap;
    use crate::prompts;

    let mut vars = HashMap::new();
    vars.insert("chapter_title", filename);
    vars.insert("chapter_text", content);
    vars.insert("bible", bible);

    let raw = prompts::execute_prompt(db, "sdt_check", provider, api_key, model, vars).await?;

    let clean = raw.trim()
        .trim_start_matches("```json").trim_start_matches("```")
        .trim_end_matches("```").trim();

    // If the model dumped reasoning before the JSON, extract just the array
    let json_str = if clean.starts_with('[') {
        clean.to_string()
    } else if let Some(start) = clean.find('[') {
        // Find the matching closing bracket
        let bytes = clean.as_bytes();
        let mut depth = 0i32;
        let mut in_string = false;
        let mut escape = false;
        let mut end = clean.len();
        for (i, &b) in bytes[start..].iter().enumerate() {
            if escape { escape = false; continue; }
            match b {
                b'\\' if in_string => escape = true,
                b'"' => in_string = !in_string,
                b'[' if !in_string => depth += 1,
                b']' if !in_string => {
                    depth -= 1;
                    if depth == 0 { end = start + i + 1; break; }
                }
                _ => {}
            }
        }
        clean[start..end].to_string()
    } else {
        clean.to_string()
    };

    // Try full parse first
    if let Ok(violations) = serde_json::from_str::<Vec<AiViolation>>(&json_str) {
        return Ok(violations.into_iter()
            .filter(|v| !v.telling_text.is_empty())
            .collect());
    }

    // Fallback: parse individually from Value array
    let arr = serde_json::from_str::<Vec<serde_json::Value>>(&json_str)
        .map_err(|e| format!("Parse error: {} | got: {}", e, &json_str[..json_str.len().min(200)]))?;

    let mut good = Vec::new();
    for item in arr {
        if let Ok(v) = serde_json::from_value::<AiViolation>(item) {
            if !v.telling_text.is_empty() {
                good.push(v);
            }
        }
    }
    Ok(good)
}

// ── Suggest fix for a show-don't-tell violation ──────────────────────────────

#[derive(serde::Deserialize)]
pub struct SuggestSdtFixRequest {
    pub provider:      String,
    pub api_key:       String,
    pub model:         String,
    pub telling_text:  String,
    pub context:       String,
    pub why:           String,
    pub chapter_title: String,
    #[serde(default)]
    pub folder:        String,
    #[serde(default)]
    pub bible_path:    String,
}

#[derive(serde::Serialize)]
pub struct SuggestSdtFixResult {
    pub success:     bool,
    pub suggestions: String,
    pub error:       String,
}

#[tauri::command]
pub async fn suggest_sdt_fix(app: AppHandle, request: SuggestSdtFixRequest) -> SuggestSdtFixResult {
    use std::collections::HashMap;

    let database = app.state::<db::Db>();
    let bible = crate::prompts::load_bible_for_story(&request.folder, &request.bible_path);

    let mut vars = HashMap::new();
    vars.insert("chapter_title", request.chapter_title.as_str());
    vars.insert("telling_text", request.telling_text.as_str());
    vars.insert("context", request.context.as_str());
    vars.insert("why", request.why.as_str());
    vars.insert("bible", bible.as_str());

    match crate::prompts::execute_prompt(
        &database, "sdt_suggest", &request.provider, &request.api_key, &request.model, vars,
    ).await {
        Ok(suggestions) => SuggestSdtFixResult { success: true, suggestions, error: String::new() },
        Err(e) => SuggestSdtFixResult { success: false, suggestions: String::new(), error: e },
    }
}
