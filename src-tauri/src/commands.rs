// commands.rs — Tauri command handlers
//
// Each pub fn tagged #[tauri::command] becomes callable from the frontend
// via invoke(). All blocking work runs on a spawn_blocking thread so the
// async Tauri runtime never stalls.

use std::time::Duration;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::cdp;

// ── Shared result types ───────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct StatusResult {
    pub running: bool,
    pub cdp_enabled: bool,
    pub page_id: String,
    pub error: String,
}

#[derive(Serialize)]
pub struct LaunchResult {
    pub success: bool,
    pub error: String,
}

#[derive(Serialize)]
pub struct AnalyzerResult {
    pub success: bool,
    pub markdown: String,
    pub error: String,
}

// ── Status ────────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn check_rocket_status() -> StatusResult {
    tokio::task::spawn_blocking(|| {
        match cdp::get_page_target() {
            Ok(t) => StatusResult {
                running: true, cdp_enabled: true,
                page_id: t.id, error: String::new(),
            },
            Err(e) => StatusResult {
                running: cdp::is_port_open(), cdp_enabled: false,
                page_id: String::new(), error: e,
            },
        }
    }).await.unwrap()
}

// ── Launch ────────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn launch_rocket() -> LaunchResult {
    tokio::task::spawn_blocking(|| {
        match cdp::ensure_rocket() {
            Ok(_) => LaunchResult { success: true, error: String::new() },
            Err(e) => LaunchResult { success: false, error: e },
        }
    }).await.unwrap()
}

// ── Category Analyzer ────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CategoryRequest {
    pub paths: Vec<String>,
}

#[tauri::command]
pub async fn analyze_categories(
    app: AppHandle,
    request: CategoryRequest,
) -> AnalyzerResult {
    tokio::task::spawn_blocking(move || {
        run_category_analyzer(&app, request.paths)
    }).await.unwrap()
}

fn emit_log(app: &AppHandle, msg: &str) {
    let _ = app.emit("cdp:log", msg);
}

fn run_category_analyzer(app: &AppHandle, paths: Vec<String>) -> AnalyzerResult {
    emit_log(app, "Connecting to Publisher Rocket...");

    let target = match cdp::ensure_rocket() {
        Ok(t) => t,
        Err(e) => return AnalyzerResult { success: false, markdown: String::new(), error: e },
    };

    let mut session = match cdp::connect(&target) {
        Ok(s) => s,
        Err(e) => return AnalyzerResult { success: false, markdown: String::new(), error: e },
    };

    emit_log(app, "CDP session established.");

    let now = chrono::Utc::now().format("%B %-d, %Y %H:%M UTC").to_string();
    let mut lines = vec![
        "# Category Research".to_string(),
        format!("Generated: {}", now),
        String::new(),
    ];

    for (ci, full_path) in paths.iter().enumerate() {
        let segments: Vec<&str> = full_path.split('>').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        if segments.is_empty() { continue; }

        emit_log(app, &format!("[{}/{}] {}", ci + 1, paths.len(), full_path));

        // Navigate to Category Search
        let nav = session.eval(r#"
            const el = Array.from(document.querySelectorAll('p,span,div,a'))
              .find(e => e.children.length === 0 && e.textContent.trim() === 'Category Search');
            if (!el) return JSON.stringify(null);
            el.scrollIntoView({block:'center'});
            const r = el.getBoundingClientRect();
            return JSON.stringify({x:Math.round(r.x+r.width/2), y:Math.round(r.y+r.height/2)});
        "#, 8);
        if let Ok(s) = nav {
            if let Ok(v) = serde_json::from_str::<Value>(&s) {
                if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                    let _ = session.click(x, y);
                }
            }
        }
        std::thread::sleep(Duration::from_secs(2));

        // Click Kindle radio
        let kindle = session.eval(r#"
            const el = Array.from(document.querySelectorAll('label,span,p,input'))
              .find(e => e.textContent && e.textContent.trim() === 'Kindle');
            if (!el) return JSON.stringify(null);
            const t = el.closest('label') || el;
            const r = t.getBoundingClientRect();
            return JSON.stringify({x:Math.round(r.x+r.width/2), y:Math.round(r.y+r.height/2)});
        "#, 8);
        if let Ok(s) = kindle {
            if let Ok(v) = serde_json::from_str::<Value>(&s) {
                if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                    let _ = session.click(x, y);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(800));

        // Type top-level category
        let top_cat = segments[0];
        let js = format!(r#"
            const input = document.querySelector('input[type="text"],input[type="search"],input:not([type="radio"]):not([type="checkbox"])');
            if (input) {{
                input.value = '';
                input.dispatchEvent(new Event('input',{{bubbles:true}}));
                input.value = {};
                input.dispatchEvent(new Event('input',{{bubbles:true}}));
                input.dispatchEvent(new Event('change',{{bubbles:true}}));
            }}
            return '';
        "#, serde_json::to_string(top_cat).unwrap());
        let _ = session.eval(&js, 8);
        std::thread::sleep(Duration::from_secs(2));

        // Click "Check it out"
        let cio_js = format!(r#"
            const rows = Array.from(document.querySelectorAll('tr'));
            for (const row of rows) {{
                const cells = row.querySelectorAll('td');
                if (cells.length > 0 && cells[0].textContent.trim().includes({})) {{
                    const btn = Array.from(row.querySelectorAll('button'))
                        .find(b => b.textContent.trim() === 'Check it out');
                    if (btn) {{
                        btn.scrollIntoView({{block:'center'}});
                        const r = btn.getBoundingClientRect();
                        return JSON.stringify({{x:Math.round(r.x+r.width/2), y:Math.round(r.y+r.height/2)}});
                    }}
                }}
            }}
            return JSON.stringify(null);
        "#, serde_json::to_string(top_cat).unwrap());

        let cio = session.eval(&cio_js, 8);
        let clicked = if let Ok(s) = cio {
            if let Ok(v) = serde_json::from_str::<Value>(&s) {
                if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                    let _ = session.click(x, y);
                    true
                } else { false }
            } else { false }
        } else { false };

        if !clicked {
            emit_log(app, &format!("  SKIP: '{}' not found in results", top_cat));
            lines.push(format!("## {}", full_path));
            lines.push(String::new());
            lines.push("*Not found*".to_string());
            lines.push(String::new());
            lines.push("---".to_string());
            lines.push(String::new());
            continue;
        }
        std::thread::sleep(Duration::from_secs(5));

        // Scrape matching row
        let target_seg = segments[segments.len() - 1];
        let row_js = format!(r#"
            const target = {}.toLowerCase();
            const rows = Array.from(document.querySelectorAll('tr')).slice(1);
            let bestRow = null, bestScore = 0;
            for (const row of rows) {{
                const cells = Array.from(row.querySelectorAll('td'));
                if (cells.length < 2) continue;
                const txt = cells[0] ? cells[0].textContent.replace(/\s+/g,' ').trim() : '';
                const lastSeg = txt.split(/[>\/]/).pop().trim().toLowerCase();
                let score = 0;
                if (lastSeg === target) score = 100;
                else if (lastSeg.startsWith(target) || target.startsWith(lastSeg)) score = 80;
                else if (txt.toLowerCase().includes(target) || target.includes(lastSeg)) score = 60;
                if (score > bestScore) {{ bestScore = score; bestRow = {{ row, cells }}; }}
            }}
            if (!bestRow || bestScore === 0) return JSON.stringify(null);
            const {{ row, cells }} = bestRow;
            const btns = Array.from(row.querySelectorAll('button'));
            const iBtn = btns.find(b => b.textContent.trim() === 'Insights');
            const kBtn = btns.find(b => b.textContent.trim() === 'Keywords');
            const ri = iBtn ? iBtn.getBoundingClientRect() : null;
            const rk = kBtn ? kBtn.getBoundingClientRect() : null;
            return JSON.stringify({{
                salesToOne:   cells[1]?cells[1].textContent.trim():'',
                salesToTen:   cells[2]?cells[2].textContent.trim():'',
                publisherPct: cells[3]?cells[3].textContent.trim():'',
                kuPct:        cells[4]?cells[4].textContent.trim():'',
                iCoords: ri?{{x:Math.round(ri.x+ri.width/2),y:Math.round(ri.y+ri.height/2)}}:null,
                kCoords: rk?{{x:Math.round(rk.x+rk.width/2),y:Math.round(rk.y+rk.height/2)}}:null,
            }});
        "#, serde_json::to_string(target_seg).unwrap());

        let row_result = session.eval(&row_js, 15);
        let row: Value = match row_result {
            Ok(s) if s != "null" && !s.is_empty() => {
                serde_json::from_str(&s).unwrap_or(Value::Null)
            }
            _ => Value::Null,
        };

        if row.is_null() {
            emit_log(app, &format!("  SKIP: '{}' not in subcategory table", target_seg));
            lines.push(format!("## {}", full_path));
            lines.push(String::new());
            lines.push("*Subcategory not found*".to_string());
            lines.push(String::new());
            lines.push("---".to_string());
            lines.push(String::new());
            // Click back
            click_back(&mut session);
            std::thread::sleep(Duration::from_secs(2));
            continue;
        }

        lines.push(format!("## {}", full_path));
        lines.push(String::new());
        lines.push(format!("- **Sales to #1:** {}", row["salesToOne"].as_str().unwrap_or("")));
        lines.push(format!("- **Sales to #10:** {}", row["salesToTen"].as_str().unwrap_or("")));
        lines.push(format!("- **Publisher %:** {}", row["publisherPct"].as_str().unwrap_or("")));
        lines.push(format!("- **KU %:** {}", row["kuPct"].as_str().unwrap_or("")));
        lines.push(String::new());

        // Insights modal
        if let (Some(x), Some(y)) = (row["iCoords"]["x"].as_f64(), row["iCoords"]["y"].as_f64()) {
            let _ = session.click(x, y);
            std::thread::sleep(Duration::from_secs(2));
            let ins = session.eval(r#"
                const o = document.querySelector('[class*="modal"],[class*="overlay"],[class*="popup"]');
                return o ? o.innerText : '';
            "#, 8).unwrap_or_default();
            if !ins.is_empty() {
                lines.push("### Insights".to_string());
                lines.push(String::new());
                lines.push(ins);
                lines.push(String::new());
            }
            session.key_escape();
        }

        // Keywords modal
        if let (Some(x), Some(y)) = (row["kCoords"]["x"].as_f64(), row["kCoords"]["y"].as_f64()) {
            let _ = session.click(x, y);
            std::thread::sleep(Duration::from_secs(2));
            let kw = session.eval(r#"
                const o = document.querySelector('[class*="modal"],[class*="overlay"],[class*="popup"]');
                return o ? o.innerText : '';
            "#, 8).unwrap_or_default();
            if !kw.is_empty() {
                lines.push("### Keywords".to_string());
                lines.push(String::new());
                lines.push(kw);
                lines.push(String::new());
            }
            session.key_escape();
        }

        lines.push("---".to_string());
        lines.push(String::new());

        click_back(&mut session);
        std::thread::sleep(Duration::from_secs(2));
    }

    emit_log(app, &format!("✓ Category analysis complete — {} categories", paths.len()));
    AnalyzerResult {
        success: true,
        markdown: lines.join("\n"),
        error: String::new(),
    }
}

fn click_back(session: &mut cdp::Session) {
    let back = session.eval(r#"
        const el = Array.from(document.querySelectorAll('button,a,span'))
          .find(e => e.textContent.trim() === 'Back' || e.textContent.trim() === '← Back');
        if (!el) return JSON.stringify(null);
        el.scrollIntoView({block:'center'});
        const r = el.getBoundingClientRect();
        return JSON.stringify({x:Math.round(r.x+r.width/2), y:Math.round(r.y+r.height/2)});
    "#, 8);
    if let Ok(s) = back {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                let _ = session.click(x, y);
            }
        }
    }
}

// ── CSV Analyzer ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CsvRequest {
    pub keyword: String,
    pub csv_content: String,
    pub api_key: String,
    pub model: String,
}

#[tauri::command]
pub async fn analyze_csv(
    app: AppHandle,
    request: CsvRequest,
) -> AnalyzerResult {
    tokio::task::spawn_blocking(move || {
        run_csv_analyzer(&app, request)
    }).await.unwrap()
}

fn run_csv_analyzer(app: &AppHandle, req: CsvRequest) -> AnalyzerResult {
    emit_log(app, &format!("Running CSV Analyzer for keyword: {}...", req.keyword));

    let system = r#"You are a publishing strategist helping an indie author analyze Amazon keyword competition data exported from Publisher Rocket.

Produce clear, actionable markdown analysis to be saved in the author's keyword research journal.

Write in plain, direct language. Focus on what the data means for a new author launching Book 1 of a series in the Christian fiction / mystery / suspense niche.

Format your response with exactly these sections:

## Competition Summary
## Key Books to Study
## What This Means for Your Book
## Verdict

Keep each section concise. No padding."#;

    let user = format!("Keyword: {}\n\nRaw CSV data:\n\n{}", req.keyword, req.csv_content);

    match call_anthropic(&req.api_key, &req.model, system, &user, 1500) {
        Ok(markdown) => {
            emit_log(app, "✓ CSV analysis complete.");
            AnalyzerResult { success: true, markdown, error: String::new() }
        }
        Err(e) => AnalyzerResult { success: false, markdown: String::new(), error: e },
    }
}

// ── AI client ─────────────────────────────────────────────────────────────────

fn call_anthropic(
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

    let resp = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .unwrap()
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .map_err(|e| format!("Anthropic request failed: {}", e))?;

    let json: Value = resp.json()
        .map_err(|e| format!("Anthropic response parse failed: {}", e))?;

    if let Some(err) = json.get("error") {
        return Err(format!("Anthropic API error: {}", err["message"].as_str().unwrap_or("unknown")));
    }

    json["content"][0]["text"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "Anthropic: empty response".to_string())
}
