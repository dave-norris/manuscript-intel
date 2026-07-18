// commands.rs — Tauri command handlers and async LLM client

use std::time::Duration;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

// ── Shared result types ───────────────────────────────────────────────────────

#[derive(Serialize, Clone, Default)]
pub struct CategoryStatRow {
    pub requested_path: String,
    pub matched_path:   String,
    pub found:          bool,
    pub sales_to_one:   String,
    pub sales_to_ten:   String,
    pub publisher_pct:  String,
    pub ku_pct:         String,
    pub top_books:      Vec<crate::canopy::TopBook>,
}

#[derive(Serialize)]
pub struct AnalyzerResult {
    pub success:  bool,
    pub markdown: String,
    pub error:    String,
    #[serde(default)]
    pub rows:     Vec<CategoryStatRow>,
}

// ── CSV Analyzer ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CsvRequest {
    pub keyword: String,
    pub csv_content: String,
    pub api_key: String,
    pub model: String,
    pub provider: String,
}

#[tauri::command]
pub async fn analyze_csv(
    app: AppHandle,
    request: CsvRequest,
) -> AnalyzerResult {
    let _ = app.emit("cdp:log", &format!("Running CSV Analyzer for keyword: {} [{}]...", request.keyword, request.model));

    let system = r#"You are a publishing strategist helping an indie author analyze Amazon keyword competition data.

Produce clear, actionable markdown analysis to be saved in the author's keyword research journal.

Write in plain, direct language. Focus on what the data means for a new author launching Book 1 of a series in the Christian fiction / mystery / suspense niche.

Format your response with exactly these sections:

## Competition Summary
## Key Books to Study
## What This Means for Your Book
## Verdict

Keep each section concise. No padding."#;

    let user = format!("Keyword: {}\n\nRaw CSV data:\n\n{}", request.keyword, request.csv_content);

    match call_llm(&request.provider, &request.api_key, &request.model, system, &user, 1500).await {
        Ok(markdown) => {
            let _ = app.emit("cdp:log", "✓ CSV analysis complete.");
            AnalyzerResult { success: true, markdown, error: String::new(), rows: Vec::new() }
        }
        Err(e) => AnalyzerResult { success: false, markdown: String::new(), error: e, rows: Vec::new() },
    }
}

// ── AI client (async) ─────────────────────────────────────────────────────────

/// Call the LLM asynchronously. Cancellable via tokio::select! — dropping the
/// future closes the HTTP connection immediately.
pub async fn call_llm(
    provider: &str,
    api_key: &str,
    model: &str,
    system: &str,
    user: &str,
    max_tokens: u32,
) -> Result<String, String> {
    match provider {
        "tokenmix" => call_tokenmix(api_key, model, system, user, max_tokens, false).await,
        _ => call_claude(api_key, model, system, user, max_tokens).await,
    }
}

/// Same as call_llm but forces JSON mode (valid JSON guaranteed in response).
pub async fn call_llm_json(
    provider: &str,
    api_key: &str,
    model: &str,
    system: &str,
    user: &str,
    max_tokens: u32,
) -> Result<String, String> {
    match provider {
        "tokenmix" => call_tokenmix(api_key, model, system, user, max_tokens, true).await,
        _ => call_claude(api_key, model, system, user, max_tokens).await,
    }
}

async fn call_claude(
    api_key: &str,
    model: &str,
    system: &str,
    user: &str,
    max_tokens: u32,
) -> Result<String, String> {
    let body = json!({
        "model": model,
        "max_tokens": max_tokens,
        "system": system,
        "messages": [{"role": "user", "content": user}]
    });

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Claude request failed: {}", e))?;

    let json: Value = resp.json()
        .await
        .map_err(|e| format!("Claude response parse failed: {}", e))?;

    if let Some(err) = json.get("error") {
        return Err(format!("Claude API error: {}", err["message"].as_str().unwrap_or("unknown")));
    }

    json["content"][0]["text"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "Claude: empty response".to_string())
}

async fn call_tokenmix(
    api_key: &str,
    model: &str,
    system: &str,
    user: &str,
    max_tokens: u32,
    json_mode: bool,
) -> Result<String, String> {
    let mut body = json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user}
        ]
    });

    if json_mode {
        body["response_format"] = json!({"type": "json_object"});
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let resp = client
        .post("https://api.tokenmix.ai/v1/chat/completions")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("TokenMix request failed: {}", e))?;

    let json: Value = resp.json()
        .await
        .map_err(|e| format!("TokenMix response parse failed: {}", e))?;

    if let Some(err) = json.get("error") {
        let msg = err["message"].as_str().unwrap_or(
            err.as_str().unwrap_or("unknown error")
        );
        return Err(format!("TokenMix error: {}", msg));
    }

    json["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "TokenMix: empty response".to_string())
}

// ── List models (async) ───────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct ModelsResult {
    pub success: bool,
    pub models: Vec<ModelInfo>,
    pub error: String,
}

#[derive(Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub owned_by: String,
    pub input_price: Option<f64>,
    pub output_price: Option<f64>,
}

#[tauri::command]
pub async fn list_models(provider: String, api_key: String) -> ModelsResult {
    match provider.as_str() {
        "tokenmix" => fetch_tokenmix_models(&api_key).await,
        "claude" => fetch_claude_models(),
        _ => ModelsResult {
            success: false, models: Vec::new(),
            error: format!("Unknown provider: {}", provider),
        },
    }
}

async fn fetch_tokenmix_models(api_key: &str) -> ModelsResult {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(e) => return ModelsResult { success: false, models: Vec::new(), error: format!("Client error: {}", e) },
    };

    // Use the new API endpoint with type=llm filter to get only chat models with pricing
    let resp = match client
        .get("https://aihubmix.com/api/v1/models?type=llm")
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => {
            // Fallback to legacy endpoint if new API fails
            return fetch_tokenmix_models_legacy(&client, api_key).await;
        }
    };

    let json: Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => return fetch_tokenmix_models_legacy(&client, api_key).await,
    };

    if let Some(err) = json.get("error") {
        let msg = err["message"].as_str().unwrap_or("unknown");
        return ModelsResult {
            success: false, models: Vec::new(),
            error: format!("API error: {}", msg),
        };
    }

    let models: Vec<ModelInfo> = json["data"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|m| {
            let id = m["model_id"].as_str()
                .or_else(|| m["id"].as_str())
                .unwrap_or("");
            if id.is_empty() { return None; }

            // Pricing: pass through raw values from API
            let input_price = m["pricing"]["input"].as_f64();
            let output_price = m["pricing"]["output"].as_f64();

            Some(ModelInfo {
                id: id.to_string(),
                owned_by: m["owned_by"].as_str()
                    .or_else(|| m["desc"].as_str().map(|d| &d[..d.len().min(40)]))
                    .unwrap_or("")
                    .to_string(),
                input_price,
                output_price,
            })
        })
        .collect();

    if models.is_empty() {
        return fetch_tokenmix_models_legacy(&client, api_key).await;
    }

    ModelsResult { success: true, models, error: String::new() }
}

/// Legacy /v1/models endpoint fallback (no pricing, no type filter)
async fn fetch_tokenmix_models_legacy(client: &reqwest::Client, api_key: &str) -> ModelsResult {
    let resp = match client
        .get("https://api.tokenmix.ai/v1/models")
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return ModelsResult {
            success: false, models: Vec::new(),
            error: format!("Request failed: {}", e),
        },
    };

    let json: Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => return ModelsResult {
            success: false, models: Vec::new(),
            error: format!("Parse failed: {}", e),
        },
    };

    if let Some(err) = json.get("error") {
        let msg = err["message"].as_str().unwrap_or("unknown");
        return ModelsResult {
            success: false, models: Vec::new(),
            error: format!("API error: {}", msg),
        };
    }

    let models: Vec<ModelInfo> = json["data"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|m| {
            let input_price = m["pricing"]["input"].as_f64()
                .or_else(|| m["pricing"]["prompt"].as_f64());
            let output_price = m["pricing"]["output"].as_f64()
                .or_else(|| m["pricing"]["completion"].as_f64());
            ModelInfo {
                id: m["id"].as_str().unwrap_or("").to_string(),
                owned_by: m["owned_by"].as_str().unwrap_or("").to_string(),
                input_price,
                output_price,
            }
        })
        .filter(|m| !m.id.is_empty())
        .collect();

    ModelsResult { success: true, models, error: String::new() }
}

fn fetch_claude_models() -> ModelsResult {
    let models = vec![
        ModelInfo { id: "claude-opus-4-20250514".to_string(), owned_by: "anthropic".to_string(), input_price: Some(15.0), output_price: Some(75.0) },
        ModelInfo { id: "claude-sonnet-4-20250514".to_string(), owned_by: "anthropic".to_string(), input_price: Some(3.0), output_price: Some(15.0) },
        ModelInfo { id: "claude-haiku-4-5-20251001".to_string(), owned_by: "anthropic".to_string(), input_price: Some(1.0), output_price: Some(5.0) },
        ModelInfo { id: "claude-3-5-sonnet-20241022".to_string(), owned_by: "anthropic".to_string(), input_price: Some(3.0), output_price: Some(15.0) },
        ModelInfo { id: "claude-3-5-haiku-20241022".to_string(), owned_by: "anthropic".to_string(), input_price: Some(0.8), output_price: Some(4.0) },
    ];
    ModelsResult { success: true, models, error: String::new() }
}

// ── Manuscript file operations ────────────────────────────────────────────────

/// Read a chapter file from disk. Returns the full text content.
/// If the exact path doesn't exist, searches recursively in the parent folder
/// for a file with the same name (handles reports that stored only the filename).
#[tauri::command]
pub async fn read_chapter(file_path: String) -> Result<String, String> {
    let path = std::path::PathBuf::from(&file_path);

    // Try the exact path first
    if path.exists() {
        return tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| format!("Could not read {}: {}", file_path, e));
    }

    // If not found, search for the filename in parent directories
    if let Some(filename) = path.file_name() {
        if let Some(parent) = path.parent() {
            // Walk up to find the story folder (try parent, then grandparent)
            for ancestor in [parent, parent.parent().unwrap_or(parent)] {
                if let Some(found) = find_file_recursive(ancestor, filename.to_str().unwrap_or("")) {
                    return tokio::fs::read_to_string(&found)
                        .await
                        .map_err(|e| format!("Could not read {}: {}", found.display(), e));
                }
            }
        }
    }

    Err(format!("Could not find {}", file_path))
}

fn find_file_recursive(dir: &std::path::Path, target_name: &str) -> Option<std::path::PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if name.starts_with('.') || name == "_analysis" { continue; }
        if path.is_dir() {
            if let Some(found) = find_file_recursive(&path, target_name) {
                return Some(found);
            }
        } else if name == target_name {
            return Some(path);
        }
    }
    None
}

/// Save a chapter file to disk (full overwrite). Used by the editor's auto-save.
#[tauri::command]
pub async fn save_chapter(file_path: String, content: String) -> Result<(), String> {
    let path = std::path::PathBuf::from(&file_path);

    // Resolve the actual path (handles old reports with just filename)
    let real_path = if path.exists() {
        path.clone()
    } else if let (Some(filename), Some(parent)) = (path.file_name(), path.parent()) {
        let name = filename.to_str().unwrap_or("");
        [parent, parent.parent().unwrap_or(parent)].iter()
            .find_map(|dir| find_file_recursive(dir, name))
            .ok_or_else(|| format!("Could not find {}", file_path))?
    } else {
        return Err(format!("Could not find {}", file_path));
    };

    tokio::fs::write(&real_path, &content)
        .await
        .map_err(|e| format!("Could not write {}: {}", real_path.display(), e))
}

/// Apply a text fix to a manuscript file on disk.
/// Finds `old_text` in the file and replaces it with `new_text`.
/// Returns the updated full content so the UI can refresh.
#[tauri::command]
pub async fn write_manuscript_fix(file_path: String, old_text: String, new_text: String) -> Result<String, String> {
    let path = std::path::PathBuf::from(&file_path);

    // Resolve the actual path (handles old reports with just filename)
    let real_path = if path.exists() {
        path.clone()
    } else if let (Some(filename), Some(parent)) = (path.file_name(), path.parent()) {
        let name = filename.to_str().unwrap_or("");
        [parent, parent.parent().unwrap_or(parent)].iter()
            .find_map(|dir| find_file_recursive(dir, name))
            .ok_or_else(|| format!("Could not find {}", file_path))?
    } else {
        return Err(format!("Could not find {}", file_path));
    };

    let content = tokio::fs::read_to_string(&real_path)
        .await
        .map_err(|e| format!("Could not read {}: {}", real_path.display(), e))?;

    if !content.contains(&old_text) {
        return Err("Could not find the original text in the file. It may have already been changed.".to_string());
    }

    let updated = content.replacen(&old_text, &new_text, 1);

    tokio::fs::write(&real_path, &updated)
        .await
        .map_err(|e| format!("Could not write {}: {}", real_path.display(), e))?;

    Ok(updated)
}

// ── Manuscript file tree ──────────────────────────────────────────────────────

#[derive(Serialize, Clone, Debug)]
pub struct FileTreeEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub children: Vec<FileTreeEntry>,
}

/// Returns the manuscript folder tree: directories and .md files, sorted naturally.
/// Skips hidden files/folders and _analysis directories.
#[tauri::command]
pub async fn list_manuscript_files(folder: String) -> Result<Vec<FileTreeEntry>, String> {
    let root = std::path::PathBuf::from(&folder);
    if !root.exists() {
        return Err(format!("Folder does not exist: {}", folder));
    }
    Ok(build_file_tree(&root))
}

fn build_file_tree(dir: &std::path::Path) -> Vec<FileTreeEntry> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };

    let mut dirs: Vec<FileTreeEntry> = Vec::new();
    let mut files: Vec<FileTreeEntry> = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();

        // Skip hidden and _analysis
        if name.starts_with('.') || name == "_analysis" { continue; }

        if path.is_dir() {
            let children = build_file_tree(&path);
            // Only include directories that contain .md files (directly or nested)
            if !children.is_empty() {
                dirs.push(FileTreeEntry {
                    name,
                    path: path.to_string_lossy().to_string(),
                    is_dir: true,
                    children,
                });
            }
        } else if path.extension().map(|e| e == "md").unwrap_or(false) {
            files.push(FileTreeEntry {
                name,
                path: path.to_string_lossy().to_string(),
                is_dir: false,
                children: Vec::new(),
            });
        }
    }

    // Natural sort both
    dirs.sort_by(|a, b| natural_sort_cmp(&a.name, &b.name));
    files.sort_by(|a, b| natural_sort_cmp(&a.name, &b.name));

    // Directories first, then files
    dirs.extend(files);
    dirs
}

fn natural_sort_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    fn key(s: &str) -> Vec<u64> {
        let mut k = Vec::new();
        let mut num = String::new();
        for c in s.chars() {
            if c.is_ascii_digit() {
                num.push(c);
            } else {
                if !num.is_empty() {
                    k.push(num.parse::<u64>().unwrap_or(0));
                    num.clear();
                }
                k.push(c.to_lowercase().next().unwrap_or(c) as u64 + 1_000_000);
            }
        }
        if !num.is_empty() { k.push(num.parse::<u64>().unwrap_or(0)); }
        k
    }
    key(a).cmp(&key(b))
}

// ── Cost estimation ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CostEstimateRequest {
    pub folder: String,
    /// Map of report_id → (input_price_per_token, output_price_per_token)
    /// Prices as returned by the API (per-1K tokens).
    pub model_prices: Vec<ReportModelPrice>,
}

#[derive(Deserialize)]
pub struct ReportModelPrice {
    pub report_id:    String,
    pub input_price:  f64,  // per 1K tokens
    pub output_price: f64,  // per 1K tokens
}

#[derive(Serialize)]
pub struct CostEstimateResult {
    pub success: bool,
    pub chapter_count: usize,
    pub total_words: usize,
    pub estimates: Vec<ReportCostEstimate>,
    pub error: String,
}

#[derive(Serialize)]
pub struct ReportCostEstimate {
    pub report_id: String,
    pub estimated_cost: f64,  // in USD
    pub calls: usize,
    pub input_tokens: usize,
    pub output_tokens: usize,
}

/// Estimate the AI cost for each report based on manuscript size and model pricing.
#[tauri::command]
pub async fn estimate_report_costs(request: CostEstimateRequest) -> CostEstimateResult {
    use crate::analysis::chapters::collect_chapters;

    let folder = std::path::PathBuf::from(&request.folder);
    if !folder.exists() {
        return CostEstimateResult {
            success: false, chapter_count: 0, total_words: 0,
            estimates: Vec::new(), error: "Folder does not exist.".to_string(),
        };
    }

    let chapters = collect_chapters(&folder);
    let chapter_count = chapters.len();

    // Count words per chapter
    let word_counts: Vec<usize> = chapters.iter().map(|p| {
        std::fs::read_to_string(p)
            .map(|c| c.split_whitespace().count())
            .unwrap_or(0)
    }).collect();
    let total_words: usize = word_counts.iter().sum();

    // Token estimation constants
    const WORDS_TO_TOKENS: f64 = 1.3;  // average for English prose
    const SYSTEM_PROMPT_TOKENS: usize = 400;  // approximate for our prompts

    // Per-report parameters: (truncation_limit_words, output_max_tokens, is_per_chapter, fixed_calls)
    struct ReportParams {
        truncation: usize,
        output_max: usize,
        per_chapter: bool,
        fixed_calls: usize,
    }

    let params_for = |report_id: &str| -> ReportParams {
        match report_id {
            "chapter_summaries" => ReportParams { truncation: 8000, output_max: 600, per_chapter: true, fixed_calls: 0 },
            "continuity_check"  => ReportParams { truncation: 6000, output_max: 4000, per_chapter: true, fixed_calls: 3 }, // extract (per ch) + judge (few batches)
            "show_dont_tell"    => ReportParams { truncation: 4000, output_max: 4000, per_chapter: true, fixed_calls: 0 },
            "genre_analysis"    => ReportParams { truncation: 0, output_max: 1200, per_chapter: false, fixed_calls: 1 },
            "genre_ranking"     => ReportParams { truncation: 0, output_max: 1200, per_chapter: false, fixed_calls: 1 },
            "kdp_categories"    => ReportParams { truncation: 0, output_max: 1200, per_chapter: false, fixed_calls: 2 },
            "bisac_classification" => ReportParams { truncation: 0, output_max: 1200, per_chapter: false, fixed_calls: 2 },
            "kdp_keywords"      => ReportParams { truncation: 0, output_max: 1200, per_chapter: false, fixed_calls: 1 },
            "mi_search_terms"   => ReportParams { truncation: 0, output_max: 300, per_chapter: false, fixed_calls: 1 },
            "discovery_keywords" => ReportParams { truncation: 0, output_max: 1200, per_chapter: false, fixed_calls: 1 },
            "zeigarnik_analysis" => ReportParams { truncation: 0, output_max: 0, per_chapter: false, fixed_calls: 0 }, // no AI
            _ => ReportParams { truncation: 4000, output_max: 1000, per_chapter: false, fixed_calls: 1 },
        }
    };

    let mut estimates = Vec::new();

    for rp in &request.model_prices {
        let params = params_for(&rp.report_id);

        // Skip non-AI reports
        if params.output_max == 0 && params.fixed_calls == 0 && !params.per_chapter {
            estimates.push(ReportCostEstimate {
                report_id: rp.report_id.clone(),
                estimated_cost: 0.0,
                calls: 0,
                input_tokens: 0,
                output_tokens: 0,
            });
            continue;
        }

        let (calls, total_input_tokens, total_output_tokens) = if params.per_chapter {
            // Per-chapter reports: sum input tokens across all chapters
            let input_tokens: usize = word_counts.iter().map(|&wc| {
                let truncated = if params.truncation > 0 { wc.min(params.truncation) } else { wc };
                (truncated as f64 * WORDS_TO_TOKENS) as usize + SYSTEM_PROMPT_TOKENS
            }).sum();
            let output_tokens = chapter_count * params.output_max;
            let calls = chapter_count + params.fixed_calls;
            (calls, input_tokens, output_tokens)
        } else {
            // Fixed-call reports: use a rough input estimate
            let input_tokens = params.fixed_calls * (2000 + SYSTEM_PROMPT_TOKENS);
            let output_tokens = params.fixed_calls * params.output_max;
            (params.fixed_calls, input_tokens, output_tokens)
        };

        // Cost = (input_tokens / 1000 * input_price) + (output_tokens / 1000 * output_price)
        let cost = (total_input_tokens as f64 / 1000.0 * rp.input_price)
                 + (total_output_tokens as f64 / 1000.0 * rp.output_price);

        estimates.push(ReportCostEstimate {
            report_id: rp.report_id.clone(),
            estimated_cost: (cost * 1000.0).round() / 1000.0,  // round to 3 decimal places
            calls,
            input_tokens: total_input_tokens,
            output_tokens: total_output_tokens,
        });
    }

    CostEstimateResult {
        success: true,
        chapter_count,
        total_words,
        estimates,
        error: String::new(),
    }
}

// ── AI Chat with context ──────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ChatMessage {
    pub role: String,   // "user" or "assistant"
    pub content: String,
}

#[derive(Deserialize)]
pub struct ChatRequest {
    pub provider:       String,
    pub api_key:        String,
    pub model:          String,
    pub message:        String,
    pub chapter_text:   String,
    pub chapter_title:  String,
    pub bible:          String,
    pub history:        Vec<ChatMessage>,
}

#[derive(Serialize)]
pub struct ChatResponse {
    pub success: bool,
    pub reply:   String,
    pub error:   String,
}

/// Contextual AI chat for the Writing panel.
/// Sends the user's message with the current chapter and bible as context.
#[tauri::command]
pub async fn chat_with_context(request: ChatRequest) -> ChatResponse {
    if request.api_key.is_empty() || request.model.is_empty() {
        return ChatResponse { success: false, reply: String::new(), error: "Set an API key and model in Settings.".to_string() };
    }

    let system = format!(
        r#"You are a fiction writing assistant. You help the author with their manuscript — brainstorming, rewrites, continuity questions, prose feedback, and anything else they ask about their story.

You have access to:
- The chapter the author is currently editing
- The story bible (character details, world rules, canon facts)

Be concise and direct. Match the author's voice when suggesting prose. If they paste text and ask for a rewrite, give the revised version ready to paste back.

{}

---

Current chapter: {}
---
{}"#,
        if request.bible.is_empty() { String::new() } else { format!("Story Bible:\n{}\n\n---", request.bible) },
        request.chapter_title,
        if request.chapter_text.len() > 12000 {
            format!("{}...[truncated]", &request.chapter_text[..12000])
        } else {
            request.chapter_text.clone()
        }
    );

    // Build messages array: system + history + new user message
    let mut messages = Vec::new();
    messages.push(json!({"role": "system", "content": system}));
    for msg in &request.history {
        messages.push(json!({"role": msg.role, "content": msg.content}));
    }
    messages.push(json!({"role": "user", "content": request.message}));

    let body = json!({
        "model": request.model,
        "max_tokens": 2000,
        "messages": messages,
    });

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
    {
        Ok(c) => c,
        Err(e) => return ChatResponse { success: false, reply: String::new(), error: format!("Client error: {}", e) },
    };

    let base_url = match request.provider.as_str() {
        "claude" => "https://api.anthropic.com/v1/messages",
        _ => "https://api.tokenmix.ai/v1/chat/completions",
    };

    // For Claude, use their native API format
    if request.provider == "claude" {
        let claude_messages: Vec<serde_json::Value> = request.history.iter()
            .map(|m| json!({"role": m.role, "content": m.content}))
            .chain(std::iter::once(json!({"role": "user", "content": request.message})))
            .collect();

        let claude_body = json!({
            "model": request.model,
            "max_tokens": 2000,
            "system": system,
            "messages": claude_messages,
        });

        let resp = match client.post(base_url)
            .header("x-api-key", &request.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&claude_body)
            .send().await
        {
            Ok(r) => r,
            Err(e) => return ChatResponse { success: false, reply: String::new(), error: format!("Request failed: {}", e) },
        };

        let json: Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => return ChatResponse { success: false, reply: String::new(), error: format!("Parse failed: {}", e) },
        };

        if let Some(err) = json.get("error") {
            return ChatResponse { success: false, reply: String::new(), error: format!("Claude: {}", err["message"].as_str().unwrap_or("unknown")) };
        }

        let reply = json["content"][0]["text"].as_str().unwrap_or("").to_string();
        return ChatResponse { success: true, reply, error: String::new() };
    }

    // OpenAI-compatible (TokenMix)
    let resp = match client.post(base_url)
        .header("Authorization", format!("Bearer {}", request.api_key))
        .header("content-type", "application/json")
        .json(&body)
        .send().await
    {
        Ok(r) => r,
        Err(e) => return ChatResponse { success: false, reply: String::new(), error: format!("Request failed: {}", e) },
    };

    let json: Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => return ChatResponse { success: false, reply: String::new(), error: format!("Parse failed: {}", e) },
    };

    if let Some(err) = json.get("error") {
        return ChatResponse { success: false, reply: String::new(), error: format!("API: {}", err["message"].as_str().unwrap_or("unknown")) };
    }

    let reply = json["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string();
    ChatResponse { success: true, reply, error: String::new() }
}
