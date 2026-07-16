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
                .or_else(|| m["pricing"]["prompt"].as_f64())
                .or_else(|| m["input_price"].as_f64())
                .or_else(|| m["price"]["input"].as_f64());
            let output_price = m["pricing"]["output"].as_f64()
                .or_else(|| m["pricing"]["completion"].as_f64())
                .or_else(|| m["output_price"].as_f64())
                .or_else(|| m["price"]["output"].as_f64());
            ModelInfo {
                id: m["id"].as_str().unwrap_or("").to_string(),
                owned_by: m["owned_by"].as_str().unwrap_or("").to_string(),
                input_price,
                output_price,
            }
        })
        .filter(|m| !m.id.is_empty())
        .collect();

    // If no pricing was found, try /v1/models/pricing
    let models = if models.iter().all(|m| m.input_price.is_none()) {
        if let Ok(pricing_models) = fetch_tokenmix_pricing(&client, api_key).await {
            models.into_iter().map(|mut m| {
                if let Some(pm) = pricing_models.iter().find(|p| p.id == m.id) {
                    m.input_price = pm.input_price;
                    m.output_price = pm.output_price;
                }
                m
            }).collect()
        } else {
            models
        }
    } else {
        models
    };

    ModelsResult { success: true, models, error: String::new() }
}

async fn fetch_tokenmix_pricing(client: &reqwest::Client, api_key: &str) -> Result<Vec<ModelInfo>, ()> {
    let resp = client
        .get("https://api.tokenmix.ai/v1/models/pricing")
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await
        .map_err(|_| ())?;

    let json: Value = resp.json().await.map_err(|_| ())?;

    let models = json["data"].as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|m| {
            let input_price = m["pricing"]["input"].as_f64()
                .or_else(|| m["pricing"]["prompt"].as_f64())
                .or_else(|| m["input_price"].as_f64())
                .or_else(|| m["price"]["input"].as_f64());
            let output_price = m["pricing"]["output"].as_f64()
                .or_else(|| m["pricing"]["completion"].as_f64())
                .or_else(|| m["output_price"].as_f64())
                .or_else(|| m["price"]["output"].as_f64());
            ModelInfo {
                id: m["id"].as_str().unwrap_or("").to_string(),
                owned_by: m["owned_by"].as_str().unwrap_or("").to_string(),
                input_price,
                output_price,
            }
        })
        .filter(|m| !m.id.is_empty())
        .collect();

    Ok(models)
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
