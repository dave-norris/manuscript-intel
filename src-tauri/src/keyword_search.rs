// keyword_search.rs — automates Publisher Rocket's Keyword Search tool.
//
// This is PR's core tool: given a seed keyword, it returns real Amazon
// search-volume estimates, a competition score, and estimated AMS earnings
// per related keyword. This replaces AI-guessed "best for discoverability"
// keywords with actual measured data — the single biggest gap this app had.
//
// Like every other PR automation in this codebase (Category Search,
// Competition Analyzer), this is a first-pass built from the tool's
// documented behavior, not a live DOM inspection. Expect selector fixes
// after the first real test run — that's the established pattern here.

use std::time::Duration;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};

use crate::cdp;
use crate::db;

#[derive(Serialize, Clone, Debug, Default)]
pub struct KeywordResult {
    pub keyword:            String,
    pub searches:           String,  // estimated monthly search volume
    pub competition:        String,  // competition score/rating
    pub estimated_earnings: String,  // estimated AMS earnings for ranking #1
}

fn emit(app: &AppHandle, msg: &str) {
    let _ = app.emit("cdp:log", msg);
}

// ── Tauri command ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct KeywordSearchRequest {
    pub folder: String,
    pub seed:   String,
}

#[derive(Serialize)]
pub struct KeywordSearchResponse {
    pub success: bool,
    pub results: Vec<KeywordResult>,
    pub error:   String,
}

#[tauri::command]
pub async fn search_pr_keywords(app: AppHandle, request: KeywordSearchRequest) -> KeywordSearchResponse {
    tokio::task::spawn_blocking(move || {
        match search_keyword(&app, &request.seed) {
            Ok(results) => {
                let database = app.state::<db::Db>();
                let conn = database.0.lock().unwrap();
                let rows: Vec<(String, String, String, String)> = results.iter()
                    .map(|r| (r.keyword.clone(), r.searches.clone(), r.competition.clone(), r.estimated_earnings.clone()))
                    .collect();
                let _ = db::replace_keyword_search_results(&conn, &request.folder, &request.seed, &rows);
                emit(&app, &format!("✓ {} keyword(s) saved to database.", results.len()));
                KeywordSearchResponse { success: true, results, error: String::new() }
            }
            Err(e) => KeywordSearchResponse { success: false, results: Vec::new(), error: e },
        }
    }).await.unwrap()
}

// ── CDP automation ───────────────────────────────────────────────────────────

/// Search one seed keyword and return every related keyword row Publisher
/// Rocket returns, with real search volume / competition / earnings data.
pub fn search_keyword(app: &AppHandle, seed: &str) -> Result<Vec<KeywordResult>, String> {
    emit(app, &format!("Connecting to Publisher Rocket for keyword search: \"{}\"...", seed));
    let target = cdp::ensure_rocket()?;
    let mut session = cdp::connect(&target)?;
    emit(app, "CDP session established.");

    navigate_to_keyword_search(&mut session)?;
    std::thread::sleep(Duration::from_secs(2));

    type_seed(&mut session, seed)?;
    trigger_search(&mut session)?;

    // This is a real network call to Amazon, not a local filter — give it
    // longer than Category Search's local filtering needs.
    emit(app, "  Waiting for live Amazon search-volume data...");
    std::thread::sleep(Duration::from_secs(7));

    let results = scrape_results_table(app, &mut session)?;
    emit(app, &format!("  {} keyword(s) returned.", results.len()));
    Ok(results)
}

fn navigate_to_keyword_search(session: &mut cdp::Session) -> Result<(), String> {
    let nav_js = r#"
        const candidates = ['Keyword Search', 'Keywords'];
        const el = Array.from(document.querySelectorAll('p,span,div,a,button'))
          .find(e => e.children.length === 0 && candidates.includes(e.textContent.trim()));
        if (!el) return JSON.stringify(null);
        el.scrollIntoView({block:'center'});
        const r = el.getBoundingClientRect();
        return JSON.stringify({x:Math.round(r.x+r.width/2), y:Math.round(r.y+r.height/2)});
    "#;
    if let Ok(s) = session.eval(nav_js, 8) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                session.click(x, y)?;
                return Ok(());
            }
        }
    }
    Err("Could not find the Keyword Search tab in Publisher Rocket — check the exact tab label and update navigate_to_keyword_search.".to_string())
}

/// PR sometimes hides the search input behind an icon until it's clicked —
/// same pattern already fixed for Category Search.
fn click_search_icon_if_present(session: &mut cdp::Session) {
    let already_visible_js = r#"
        const input = document.querySelector(
            'input[type="text"],input[type="search"],input:not([type="radio"]):not([type="checkbox"])'
        );
        return JSON.stringify(!!(input && input.offsetParent !== null));
    "#;
    if let Ok(s) = session.eval(already_visible_js, 5) {
        if s.trim() == "true" { return; }
    }

    let find_icon_js = r#"
        const btn = Array.from(document.querySelectorAll('button,span,a,div'))
          .find(e => {
            const txt = e.textContent.trim();
            const cls = (e.className || '').toLowerCase();
            const w = e.getBoundingClientRect().width;
            return txt === '' && (
              cls.includes('search') || cls.includes('magnif') ||
              e.querySelector('svg,img') !== null
            ) && w < 60 && w > 0;
          });
        if (btn) {
          btn.scrollIntoView({block:'center'});
          const r = btn.getBoundingClientRect();
          return JSON.stringify({x:Math.round(r.x+r.width/2), y:Math.round(r.y+r.height/2)});
        }
        return JSON.stringify(null);
    "#;
    if let Ok(s) = session.eval(find_icon_js, 8) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                let _ = session.click(x, y);
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
}

fn type_seed(session: &mut cdp::Session, seed: &str) -> Result<(), String> {
    click_search_icon_if_present(session);

    let seed_json = serde_json::to_string(seed).unwrap();
    let type_js = format!(r#"
        const input = document.querySelector(
            'input[type="text"],input[type="search"],input:not([type="radio"]):not([type="checkbox"])'
        );
        if (input) {{
            input.focus();
            input.value = '';
            input.dispatchEvent(new Event('input', {{bubbles:true}}));
            const nativeInputValueSetter = Object.getOwnPropertyDescriptor(
                window.HTMLInputElement.prototype, 'value'
            ).set;
            nativeInputValueSetter.call(input, {seed});
            input.dispatchEvent(new Event('input', {{bubbles:true}}));
            input.dispatchEvent(new Event('change', {{bubbles:true}}));
        }}
        return input ? 'ok' : 'no input found';
    "#, seed = seed_json);

    let result = session.eval(&type_js, 8)?;
    if result.contains("no input") {
        return Err("Could not find the keyword search input field — Publisher Rocket's Keyword Search UI may differ from what this expects.".to_string());
    }
    std::thread::sleep(Duration::from_millis(500));
    Ok(())
}

fn trigger_search(session: &mut cdp::Session) -> Result<(), String> {
    // Try a "Search" button first; fall back to pressing Enter in the field.
    let btn_js = r#"
        const btn = Array.from(document.querySelectorAll('button'))
          .find(b => /search/i.test(b.textContent.trim()) && b.textContent.trim().length < 20);
        if (!btn) return JSON.stringify(null);
        btn.scrollIntoView({block:'center'});
        const r = btn.getBoundingClientRect();
        return JSON.stringify({x:Math.round(r.x+r.width/2), y:Math.round(r.y+r.height/2)});
    "#;
    if let Ok(s) = session.eval(btn_js, 8) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                session.click(x, y)?;
                return Ok(());
            }
        }
    }

    // Fallback: Enter key in the field.
    let _ = session.eval(r#"
        const input = document.querySelector('input[type="text"],input[type="search"]');
        if (input) {
            const ev = new KeyboardEvent('keydown', {key:'Enter', code:'Enter', bubbles:true});
            input.dispatchEvent(ev);
        }
        return '';
    "#, 5);
    Ok(())
}

/// Scrape the keyword results table. Column order is detected from the
/// header row where possible (looking for "search"/"volum", "compet",
/// "earn"/"cpc" in header text); falls back to positional columns
/// (keyword, searches, competition, earnings) if headers aren't matched.
/// Emits a DIAG line with the raw header + first rows so a failed first
/// attempt is debuggable from the log alone, same as Category Search.
fn scrape_results_table(app: &AppHandle, session: &mut cdp::Session) -> Result<Vec<KeywordResult>, String> {
    let scrape_js = r#"
        const table = document.querySelector('table') ||
                      document.querySelector('[role="table"]');
        const allRows = table
            ? Array.from(table.querySelectorAll('tr'))
            : Array.from(document.querySelectorAll('tr'));
        if (allRows.length === 0) return JSON.stringify({ header: [], rows: [] });

        const headerCells = Array.from(allRows[0].querySelectorAll('th,td'))
            .map(c => c.textContent.trim().toLowerCase());

        const dataRows = allRows.slice(1)
            .map(r => Array.from(r.querySelectorAll('td')).map(c => c.textContent.trim()))
            .filter(cells => cells.length > 0 && cells.some(c => c.length > 0));

        return JSON.stringify({ header: headerCells, rows: dataRows.slice(0, 300) });
    "#;

    let raw = session.eval(scrape_js, 10)?;
    emit(app, &format!("  DIAG table scrape: {}", &raw[..raw.len().min(500)]));

    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| format!("Could not parse keyword table: {} | got: {}", e, &raw[..raw.len().min(200)]))?;

    let header: Vec<String> = parsed["header"].as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    let find_col = |needles: &[&str]| -> Option<usize> {
        header.iter().position(|h| needles.iter().any(|n| h.contains(n)))
    };
    let idx_keyword     = find_col(&["keyword", "phrase", "term"]).unwrap_or(0);
    let idx_searches    = find_col(&["search", "volum"]).unwrap_or(1);
    let idx_competition = find_col(&["compet"]).unwrap_or(2);
    let idx_earnings    = find_col(&["earn", "cpc", "$"]).unwrap_or(3);

    let rows = parsed["rows"].as_array().cloned().unwrap_or_default();
    let results: Vec<KeywordResult> = rows.iter().filter_map(|row| {
        let cells: Vec<String> = row.as_array()?.iter()
            .filter_map(|c| c.as_str().map(String::from)).collect();
        if cells.is_empty() { return None; }
        Some(KeywordResult {
            keyword:            cells.get(idx_keyword).cloned().unwrap_or_default(),
            searches:           cells.get(idx_searches).cloned().unwrap_or_default(),
            competition:        cells.get(idx_competition).cloned().unwrap_or_default(),
            estimated_earnings: cells.get(idx_earnings).cloned().unwrap_or_default(),
        })
    }).filter(|r| !r.keyword.is_empty()).collect();

    Ok(results)
}
