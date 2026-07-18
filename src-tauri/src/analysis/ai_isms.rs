// analysis/ai_isms.rs — AI-assisted check for AI-sounding prose habits.
//
// Mirrors Show Don't Tell: per-chapter LLM scan, JSON report with flagged
// passages + context, plus a suggest-fix command for rewrites.

use std::path::PathBuf;
use tauri::{AppHandle, Manager};

use super::{emit, err, GenreResult};
use super::chapters::{collect_chapters, extract_title};
use crate::db;

#[derive(serde::Deserialize)]
pub struct AiIsmsRequest {
    pub folder:   String,
    pub provider: String,
    pub api_key:  String,
    pub model:    String,
    #[serde(default)]
    pub bible_path: String,
}

#[derive(serde::Deserialize, Clone, Debug)]
struct AiViolation {
    #[serde(default)]
    telling_text: String,
    #[serde(default)]
    context:      String,
    #[serde(default)]
    why:          String,
    #[serde(default)]
    severity:     String,
}

#[tauri::command]
pub async fn check_ai_isms(app: AppHandle, request: AiIsmsRequest) -> GenreResult {
    let cancel = crate::cancel_notify();
    tokio::select! {
        result = check_inner(app, request) => result,
        _ = cancel.notified() => err("Cancelled."),
    }
}

async fn check_inner(app: AppHandle, request: AiIsmsRequest) -> GenreResult {
    let folder = PathBuf::from(&request.folder);
    if !folder.exists() { return err("Folder does not exist."); }
    if request.api_key.is_empty() || request.model.is_empty() {
        return err("AI-isms requires an API key and model. Set them in Settings.");
    }

    crate::reset_cancel();
    let database = app.state::<db::Db>();
    let run_ts = chrono::Utc::now().to_rfc3339();

    let chapters = collect_chapters(&folder);
    if chapters.is_empty() { return err("No .md chapter files found."); }

    let bible = crate::prompts::load_bible_for_story(&request.folder, &request.bible_path);

    emit(&app, &format!("Checking {} chapter(s) for AI-isms...", chapters.len()));

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

        let processed = {
            let conn = database.0.lock().unwrap();
            crate::prompts::get_preprocessed(&conn, &request.folder, &filename, "ai_isms_check", path)
                .unwrap_or_else(|| {
                    let p = crate::prompts::preprocess_for_ai_isms(&content);
                    crate::prompts::store_preprocessed(&conn, &request.folder, &filename, "ai_isms_check", &p, path);
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
            emit(&app, &format!("  → {} — {} flag(s)", filename, violations.len()));
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

    emit(&app, &format!("✓ AI-isms complete — {} flag(s) across {} chapter(s).",
        total_violations, all_findings.len()));

    let report = serde_json::json!({
        "schema": "ai_isms_v1",
        "note": "AI-assisted: the model flags prose habits that often read as machine-generated. Severity is subjective — use as a prompt to revise, not a verdict.",
        "summary": {
            "chapters_checked": chapters.len(),
            "chapters_with_violations": all_findings.len(),
            "total_violations": total_violations,
        },
        "chapters": all_findings,
    }).to_string();

    {
        let conn = database.0.lock().unwrap();
        let _ = db::save_document_at(&conn, &request.folder, "ai_isms", &report, &run_ts);
    }

    GenreResult { success: true, report: String::new(), error: String::new(), run_ts }
}

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

    let raw = prompts::execute_prompt(db, "ai_isms_check", provider, api_key, model, vars).await?;

    let clean = raw.trim()
        .trim_start_matches("```json").trim_start_matches("```")
        .trim_end_matches("```").trim();

    let json_str = if clean.starts_with('[') {
        clean.to_string()
    } else if let Some(start) = clean.find('[') {
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

    if let Ok(violations) = serde_json::from_str::<Vec<AiViolation>>(&json_str) {
        return Ok(violations.into_iter()
            .filter(|v| !v.telling_text.is_empty())
            .collect());
    }

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

#[derive(serde::Deserialize)]
pub struct SuggestAiIsmsFixRequest {
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
pub struct SuggestAiIsmsFixResult {
    pub success:     bool,
    pub suggestions: String,
    pub error:       String,
}

#[tauri::command]
pub async fn suggest_ai_isms_fix(app: AppHandle, request: SuggestAiIsmsFixRequest) -> SuggestAiIsmsFixResult {
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
        &database, "ai_isms_suggest", &request.provider, &request.api_key, &request.model, vars,
    ).await {
        Ok(suggestions) => SuggestAiIsmsFixResult { success: true, suggestions, error: String::new() },
        Err(e) => SuggestAiIsmsFixResult { success: false, suggestions: String::new(), error: e },
    }
}
