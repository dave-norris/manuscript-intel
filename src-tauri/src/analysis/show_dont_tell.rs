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
use super::chapters::{collect_chapters, extract_title, truncate_words};
use crate::commands::call_llm_json;
use crate::db;

// ── Request ──────────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct ShowDontTellRequest {
    pub folder:   String,
    pub provider: String,
    pub api_key:  String,
    pub model:    String,
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

    emit(&app, &format!("Checking {} chapter(s) for show-don't-tell violations...", chapters.len()));

    let mut all_findings: Vec<serde_json::Value> = Vec::new();
    let mut total_violations = 0usize;

    for (i, path) in chapters.iter().enumerate() {
        if crate::is_cancelled() { return err("Cancelled."); }

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let filename = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let title = extract_title(&content).unwrap_or_else(|| filename.clone());
        let truncated = truncate_words(&content, 4000);

        emit(&app, &format!("[{}/{}] {} — checking...", i + 1, chapters.len(), filename));

        let violations = match extract_violations(
            &request.provider, &request.api_key, &request.model,
            &filename, &truncated,
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
    provider: &str, api_key: &str, model: &str,
    filename: &str, content: &str,
) -> Result<Vec<AiViolation>, String> {
    let system = r#"You are a fiction editor checking for "telling" instead of "showing."

TELLING means the author directly states emotions, judgments, or internal states rather than letting the reader infer them from action, dialogue, body language, or sensory detail.

Examples of telling:
- "She felt nervous." (states emotion directly)
- "He was a kind man." (states judgment)
- "The party was boring." (states conclusion)
- "She realized he was lying." (tells the realization instead of showing the clues)

NOT violations (these are showing):
- "Her hands trembled as she reached for the door." (physical action implies nerves)
- "He always remembered birthdays." (behavior implies kindness)
- Dialogue, action scenes, sensory descriptions

For each violation found, return:
- telling_text: the exact words from the manuscript that tell (keep short, one sentence max)
- context: 1-2 sentences of surrounding text so the author can locate it
- why: one sentence explaining what is being told instead of shown
- severity: "minor" (quick fix), "moderate" (weakens the scene), or "major" (undermines a key moment)

Return ONLY a JSON array. No markdown, no preamble. If no violations are found, return [].
Maximum 10 violations per chapter — focus on the most impactful ones."#;

    let user = format!("Chapter: {}\n\n---\n\n{}", filename, content);

    let raw = call_llm_json(provider, api_key, model, system, &user, 4000).await?;

    let clean = raw.trim()
        .trim_start_matches("```json").trim_start_matches("```")
        .trim_end_matches("```").trim();

    // Try full parse first
    if let Ok(violations) = serde_json::from_str::<Vec<AiViolation>>(clean) {
        return Ok(violations.into_iter()
            .filter(|v| !v.telling_text.is_empty())
            .collect());
    }

    // Fallback: parse individually from Value array
    let arr = serde_json::from_str::<Vec<serde_json::Value>>(clean)
        .map_err(|e| format!("Parse error: {} | got: {}", e, &clean[..clean.len().min(200)]))?;

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
}

#[derive(serde::Serialize)]
pub struct SuggestSdtFixResult {
    pub success:     bool,
    pub suggestions: String,
    pub error:       String,
}

#[tauri::command]
pub async fn suggest_sdt_fix(request: SuggestSdtFixRequest) -> SuggestSdtFixResult {
    use crate::commands::call_llm;

    let system = r#"You are a fiction editor helping an author rewrite a "telling" passage to "show" instead. You will be given:
- The passage that tells instead of shows
- Surrounding context
- Why it's considered telling

Provide 2-3 alternative rewrites that SHOW instead of TELL. For each:
1. Give the revised prose (ready to paste — match the author's voice and tense)
2. One sentence explaining the technique used (body language, sensory detail, action, dialogue, etc.)

Keep rewrites concise — replace only the telling passage, not the surrounding context. Maintain the author's style and point of view."#;

    let user = format!(
        "Chapter: {}\n\nTelling passage: \"{}\"\n\nContext: \"{}\"\n\nWhy it's telling: {}",
        request.chapter_title, request.telling_text, request.context, request.why
    );

    match call_llm(&request.provider, &request.api_key, &request.model, system, &user, 1500).await {
        Ok(suggestions) => SuggestSdtFixResult { success: true, suggestions, error: String::new() },
        Err(e) => SuggestSdtFixResult { success: false, suggestions: String::new(), error: e },
    }
}
