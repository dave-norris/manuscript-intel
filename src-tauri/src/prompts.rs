// prompts.rs — Prompt system: load templates from DB, preprocess text, fill placeholders, call LLM.
//
// The prompt pipeline:
//   1. Load template from prompt_templates table by id
//   2. Load/generate preprocessed chapter text (cached in preprocessed_chapters)
//   3. Load bible text (if bible_path is set)
//   4. Fill placeholders in the user_template
//   5. Call LLM with system_prompt + filled user_template

use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::path::Path;

use crate::commands::{call_llm, call_llm_json};
use crate::db::Db;

// ── Template loading ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PromptTemplate {
    pub id:            String,
    pub system_prompt: String,
    pub user_template: String,
    pub max_tokens:    u32,
    pub json_mode:     bool,
}

/// Load a prompt template from the database by id.
pub fn load_template(conn: &Connection, template_id: &str) -> Result<PromptTemplate, String> {
    conn.query_row(
        "SELECT id, system_prompt, user_template, max_tokens, json_mode FROM prompt_templates WHERE id = ?1",
        params![template_id],
        |r| Ok(PromptTemplate {
            id:            r.get(0)?,
            system_prompt: r.get(1)?,
            user_template: r.get(2)?,
            max_tokens:    r.get::<_, i64>(3)? as u32,
            json_mode:     r.get::<_, i64>(4)? != 0,
        }),
    ).map_err(|e| format!("Prompt template '{}' not found: {}", template_id, e))
}

// ── Bible loading ─────────────────────────────────────────────────────────────

/// Load bible text from a file path. Returns empty string if path is empty or file doesn't exist.
pub fn load_bible(bible_path: &str) -> String {
    if bible_path.is_empty() { return String::new(); }
    let path = Path::new(bible_path);
    if !path.exists() { return String::new(); }
    match std::fs::read_to_string(path) {
        Ok(text) => {
            // Truncate to a reasonable size for context (8000 words max)
            let words: Vec<&str> = text.split_whitespace().collect();
            if words.len() > 8000 {
                words[..8000].join(" ") + "\n[Bible truncated]"
            } else {
                text
            }
        }
        Err(_) => String::new(),
    }
}

// ── Preprocessed text cache ───────────────────────────────────────────────────

/// Get or create preprocessed text for a chapter+report_type combo.
/// Returns cached version if the source file hasn't changed.
pub fn get_preprocessed(
    conn: &Connection,
    story_folder: &str,
    chapter_file: &str,
    report_type: &str,
    source_path: &Path,
) -> Option<String> {
    let file_mtime = get_file_mtime(source_path);

    // Check cache
    let cached: Option<(String, String)> = conn.query_row(
        "SELECT processed_text, source_modified_at FROM preprocessed_chapters
         WHERE story_folder = ?1 AND chapter_file = ?2 AND report_type = ?3",
        params![story_folder, chapter_file, report_type],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).ok();

    if let Some((text, cached_mtime)) = cached {
        if cached_mtime == file_mtime {
            return Some(text);
        }
    }
    None
}

/// Store preprocessed text in the cache.
pub fn store_preprocessed(
    conn: &Connection,
    story_folder: &str,
    chapter_file: &str,
    report_type: &str,
    processed_text: &str,
    source_path: &Path,
) {
    let file_mtime = get_file_mtime(source_path);
    let now = chrono::Utc::now().to_rfc3339();
    let _ = conn.execute(
        "INSERT INTO preprocessed_chapters (story_folder, chapter_file, report_type, processed_text, source_modified_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(story_folder, chapter_file, report_type)
         DO UPDATE SET processed_text = excluded.processed_text, source_modified_at = excluded.source_modified_at, created_at = excluded.created_at",
        params![story_folder, chapter_file, report_type, processed_text, file_mtime, now],
    );
}

fn get_file_mtime(path: &Path) -> String {
    path.metadata()
        .and_then(|m| m.modified())
        .map(|t| {
            let dur = t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
            format!("{}", dur.as_secs())
        })
        .unwrap_or_default()
}

// ── Placeholder filling ───────────────────────────────────────────────────────

/// Fill placeholders in a template string. Placeholders are {key} format.
/// Any unfilled placeholder is replaced with empty string.
pub fn fill_template(template: &str, vars: &HashMap<&str, &str>) -> String {
    let mut result = template.to_string();
    for (key, value) in vars {
        result = result.replace(&format!("{{{}}}", key), value);
    }
    // Remove any unfilled placeholders
    let re_unfilled = regex::Regex::new(r"\{[a-z_]+\}").unwrap();
    re_unfilled.replace_all(&result, "").to_string()
}

// ── Execute a prompt ──────────────────────────────────────────────────────────

/// Full prompt execution: load template, fill variables, call LLM.
pub async fn execute_prompt(
    db: &Db,
    template_id: &str,
    provider: &str,
    api_key: &str,
    model: &str,
    vars: HashMap<&str, &str>,
) -> Result<String, String> {
    let template = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        load_template(&conn, template_id)?
    };

    let user_content = fill_template(&template.user_template, &vars);

    if template.json_mode {
        call_llm_json(provider, api_key, model, &template.system_prompt, &user_content, template.max_tokens).await
    } else {
        call_llm(provider, api_key, model, &template.system_prompt, &user_content, template.max_tokens).await
    }
}

// ── Chapter preprocessing functions ───────────────────────────────────────────

/// Preprocess chapter text for continuity extraction: keep full text but truncate at 4000 words.
#[allow(dead_code)]
pub fn preprocess_for_continuity(content: &str) -> String {
    truncate_words(content, 4000)
}

/// Preprocess chapter text for show-don't-tell checking: keep prose, truncate at 4000 words.
pub fn preprocess_for_sdt(content: &str) -> String {
    truncate_words(content, 4000)
}

/// Preprocess chapter text for genre summary: aggressive truncation to 2000 words.
#[allow(dead_code)]
pub fn preprocess_for_genre(content: &str) -> String {
    truncate_words(content, 2000)
}

fn truncate_words(text: &str, max: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() <= max { return text.to_string(); }
    words[..max].join(" ")
}
