// commands.rs — Tauri command handlers

use std::time::Duration;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::cdp;
use crate::models;

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
    pub store: String,
    pub filter: String,
}

#[tauri::command]
pub async fn analyze_categories(
    app: AppHandle,
    request: CategoryRequest,
) -> AnalyzerResult {
    tokio::task::spawn_blocking(move || {
        run_category_analyzer(&app, request.paths, request.store, request.filter)
    }).await.unwrap()
}

fn emit_log(app: &AppHandle, msg: &str) {
    let _ = app.emit("cdp:log", msg);
}

fn run_category_analyzer(app: &AppHandle, paths: Vec<String>, store: String, filter: String) -> AnalyzerResult {
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
        let segments: Vec<&str> = full_path.split('>')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if segments.is_empty() { continue; }

        emit_log(app, &format!("[{}/{}] {}", ci + 1, paths.len(), full_path));

        // ── Navigate to Category Search ──────────────────────────────────────
        let nav_js = r#"
            const el = Array.from(document.querySelectorAll('p,span,div,a'))
              .find(e => e.children.length === 0 && e.textContent.trim() === 'Category Search');
            if (!el) return JSON.stringify(null);
            el.scrollIntoView({block:'center'});
            const r = el.getBoundingClientRect();
            return JSON.stringify({x:Math.round(r.x+r.width/2), y:Math.round(r.y+r.height/2)});
        "#;
        if let Ok(s) = session.eval(nav_js, 8) {
            if let Ok(v) = serde_json::from_str::<Value>(&s) {
                if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                    let _ = session.click(x, y);
                }
            }
        }
        std::thread::sleep(Duration::from_secs(2));

        // ── Click the selected store radio ──────────────────────────────────
        let store_label = store.as_str();
        let store_js = format!(r#"
            const el = Array.from(document.querySelectorAll('label,span,p,input'))
              .find(e => e.textContent && e.textContent.trim() === {s});
            if (!el) return JSON.stringify(null);
            const t = el.closest('label') || el;
            const r = t.getBoundingClientRect();
            return JSON.stringify({{x:Math.round(r.x+r.width/2), y:Math.round(r.y+r.height/2)}});
        "#, s = serde_json::to_string(store_label).unwrap());
        if let Ok(s) = session.eval(&store_js, 8) {
            if let Ok(v) = serde_json::from_str::<Value>(&s) {
                if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                    let _ = session.click(x, y);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(800));

        // ── Click the filter button ────────────────────────────────────
        // Publisher Rocket shows a filter button whose label matches the filter name.
        // "All" means no click needed — leave whatever is currently active.
        if filter.as_str() != "All" {
            let filter_label = filter.as_str();
            let filter_js = format!(r#"
                const btn = Array.from(document.querySelectorAll('button,span,div'))
                  .find(e => e.children.length === 0 && e.textContent.trim() === {f});
                if (!btn) return JSON.stringify(null);
                btn.scrollIntoView({{block:'center'}});
                const r = btn.getBoundingClientRect();
                return JSON.stringify({{x:Math.round(r.x+r.width/2), y:Math.round(r.y+r.height/2)}});
            "#, f = serde_json::to_string(filter_label).unwrap());
            if let Ok(s) = session.eval(&filter_js, 8) {
                if let Ok(v) = serde_json::from_str::<Value>(&s) {
                    if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                        let _ = session.click(x, y);
                        std::thread::sleep(Duration::from_millis(600));
                    }
                }
            }
        }

        // ── Type top-level category into search ──────────────────────────────
        // Use the first word before '&' or ',' as the search term — Publisher
        // Rocket's search filters by prefix so a single distinctive word is
        // enough to surface the right row, and avoids React event issues with
        // special characters like '&' in programmatically set input values.
        let top_cat = segments[0];
        let search_term = top_cat
            .split(|c| c == '&' || c == ',')
            .next()
            .unwrap_or(top_cat)
            .trim()
            .to_string();
        let top_cat_json = serde_json::to_string(top_cat).unwrap();
        let search_json  = serde_json::to_string(&search_term).unwrap();

        // PR sometimes hides the search input behind an icon until it's clicked.
        click_search_icon_if_present(&mut session);

        // Clear the field first, then type each character to trigger React's
        // synthetic onChange — direct .value assignment is not reliable in Electron.
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
                nativeInputValueSetter.call(input, {search});
                input.dispatchEvent(new Event('input', {{bubbles:true}}));
                input.dispatchEvent(new Event('change', {{bubbles:true}}));
            }}
            return '';
        "#, search = search_json);
        let _ = session.eval(&type_js, 8);
        std::thread::sleep(Duration::from_secs(2));

        // ── Diagnostic: log what rows are actually visible after search ──────
        let diag_js = r#"
            const rows = Array.from(document.querySelectorAll('tr'));
            const data = rows.map(row => {{
                const cells = Array.from(row.querySelectorAll('td'));
                const btns  = Array.from(row.querySelectorAll('button')).map(b => b.textContent.trim());
                return {{ text: cells[0]?.textContent.trim() || '', btns }};
            }}).filter(r => r.text || r.btns.length);
            return JSON.stringify(data.slice(0, 10));
        "#;
        if let Ok(diag) = session.eval(diag_js, 8) {
            emit_log(app, &format!("  DIAG rows after search: {}", diag));
        }

        // ── Also log the current input value to confirm it was set ───────────
        let val_js = r#"
            const input = document.querySelector(
                'input[type="text"],input[type="search"],input:not([type="radio"]):not([type="checkbox"])'
            );
            return input ? input.value : 'NO INPUT FOUND';
        "#;
        if let Ok(val) = session.eval(val_js, 8) {
            emit_log(app, &format!("  DIAG input value: {}", val));
        }

        // ── Find best-matching "Check it out" row and click it ───────────────
        // Always click into the subcategory table regardless of segment count —
        // the stats live there, not on the search results page.
        let cio_js = format!(r#"
            const wanted = {top}.toLowerCase();
            const rows = Array.from(document.querySelectorAll('tr'));
            let bestBtn = null, bestScore = 0, bestName = '';
            for (const row of rows) {{
                const cells = row.querySelectorAll('td');
                if (cells.length === 0) continue;
                const name = cells[0].textContent.trim().toLowerCase();
                const btn = Array.from(row.querySelectorAll('button'))
                    .find(b => b.textContent.trim() === 'Check it out');
                if (!btn) continue;
                let score = 0;
                if (name === wanted)                                          score = 100;
                else if (name.startsWith(wanted) || wanted.startsWith(name)) score = 80;
                else if (name.includes(wanted) || wanted.includes(name))     score = 60;
                else {{
                    const s = wanted.length < name.length ? wanted : name;
                    const l = wanted.length < name.length ? name  : wanted;
                    let ov = 0;
                    for (let i = 0; i < s.length; i++) {{ if (l.includes(s[i])) ov++; }}
                    score = Math.round((ov / l.length) * 40);
                }}
                if (score > bestScore) {{
                    bestScore = score; bestBtn = btn;
                    bestName = cells[0].textContent.trim();
                }}
            }}
            if (!bestBtn) return JSON.stringify(null);
            bestBtn.scrollIntoView({{block:'center'}});
            const r = bestBtn.getBoundingClientRect();
            return JSON.stringify({{
                x: Math.round(r.x + r.width/2),
                y: Math.round(r.y + r.height/2),
                name: bestName,
                score: bestScore
            }});
        "#, top = top_cat_json);

        let (top_clicked, top_matched_name, top_score) = match session.eval(&cio_js, 8) {
            Ok(ref s) if s != "null" && !s.is_empty() => {
                match serde_json::from_str::<Value>(s) {
                    Ok(v) => {
                        if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                            let name  = v["name"].as_str().unwrap_or(top_cat).to_string();
                            let score = v["score"].as_u64().unwrap_or(0);
                            let _ = session.click(x, y);
                            (true, name, score)
                        } else { (false, String::new(), 0) }
                    }
                    Err(_) => (false, String::new(), 0),
                }
            }
            _ => (false, String::new(), 0),
        };

        if !top_clicked {
            emit_log(app, &format!("  ⚠ No rows at all for '{}'", top_cat));
            lines.push(format!("## {}", full_path));
            lines.push(String::new());
            lines.push(format!(
                "*⚠️ Publisher Rocket returned no category rows for '{}'. \
                 Try a shorter or different top-level term.*",
                top_cat
            ));
            lines.push(String::new());
            lines.push("---".to_string());
            lines.push(String::new());
            continue;
        }

        if top_score < 100 {
            emit_log(app, &format!(
                "  ⚠ Top-level fuzzy match: '{}' → '{}' (score {})",
                top_cat, top_matched_name, top_score
            ));
        }
        std::thread::sleep(Duration::from_secs(5));

        // ── Find best-matching subcategory row ───────────────────────────────
        // For a single-segment path the user wants the parent category itself.
        // Publisher Rocket shows it as the first row in the subcategory table
        // with a path like "Kindle Books > Literature & Fiction". We match
        // against the LAST segment of each row's path so "Literature & Fiction"
        // matches that row exactly rather than fuzzy-matching a deep subcategory.
        //
        // For multi-segment paths we match against the last segment as before.
        let target_seg = segments[segments.len() - 1];
        let target_json = serde_json::to_string(target_seg).unwrap();

        let row_js = format!(r#"
            const target = {tgt}.toLowerCase();
            const rows = Array.from(document.querySelectorAll('tr')).slice(1);
            let bestCells = null, bestScore = 0, bestTxt = '';
            for (const row of rows) {{
                const cells = Array.from(row.querySelectorAll('td'));
                if (cells.length < 2) continue;
                const txt = cells[0] ? cells[0].textContent.replace(/\s+/g,' ').trim() : '';
                // Always compare against the LAST segment of the row path
                const lastSeg = txt.split('>').pop().trim().toLowerCase();
                let score = 0;
                if (lastSeg === target)                                               score = 100;
                else if (lastSeg.startsWith(target) || target.startsWith(lastSeg))   score = 80;
                else if (lastSeg.includes(target)   || target.includes(lastSeg))     score = 60;
                else {{
                    const s = target.length < lastSeg.length ? target : lastSeg;
                    const l = target.length < lastSeg.length ? lastSeg : target;
                    let ov = 0;
                    for (let i = 0; i < s.length; i++) {{ if (l.includes(s[i])) ov++; }}
                    score = Math.round((ov / l.length) * 40);
                }}
                if (score > bestScore) {{ bestScore = score; bestCells = cells; bestTxt = txt; }}
            }}
            if (!bestCells) return JSON.stringify(null);
            const matchRow = Array.from(document.querySelectorAll('tr')).find(r => {{
                const c = r.querySelector('td');
                return c && c.textContent.replace(/\s+/g,' ').trim() === bestTxt;
            }});
            const btns = matchRow ? Array.from(matchRow.querySelectorAll('button')) : [];
            const iBtn = btns.find(b => b.textContent.trim() === 'Insights');
            const kBtn = btns.find(b => b.textContent.trim() === 'Keywords');
            const ri = iBtn ? iBtn.getBoundingClientRect() : null;
            const rk = kBtn ? kBtn.getBoundingClientRect() : null;
            return JSON.stringify({{
                matchedPath:  bestTxt,
                score:        bestScore,
                salesToOne:   bestCells[1] ? bestCells[1].textContent.trim() : '',
                salesToTen:   bestCells[2] ? bestCells[2].textContent.trim() : '',
                publisherPct: bestCells[3] ? bestCells[3].textContent.trim() : '',
                kuPct:        bestCells[4] ? bestCells[4].textContent.trim() : '',
                iCoords: ri ? {{x:Math.round(ri.x+ri.width/2), y:Math.round(ri.y+ri.height/2)}} : null,
                kCoords: rk ? {{x:Math.round(rk.x+rk.width/2), y:Math.round(rk.y+rk.height/2)}} : null,
            }});
        "#, tgt = target_json);

        let row: Value = match session.eval(&row_js, 15) {
            Ok(ref s) if s != "null" && !s.is_empty() => {
                serde_json::from_str(s).unwrap_or(Value::Null)
            }
            _ => Value::Null,
        };

        if row.is_null() {
            emit_log(app, &format!("  ⚠ Subcategory table empty for '{}'", target_seg));
            lines.push(format!("## {}", full_path));
            lines.push(String::new());
            if top_score < 100 {
                lines.push(format!(
                    "*⚠️ Top-level matched to '{}' (not exact).*", top_matched_name
                ));
                lines.push(String::new());
            }
            lines.push(format!(
                "*⚠️ No subcategories listed under '{}'. \
                 The category may use a different name — try browsing Publisher Rocket manually.*",
                top_matched_name
            ));
            lines.push(String::new());
            lines.push("---".to_string());
            lines.push(String::new());
            click_back(&mut session);
            std::thread::sleep(Duration::from_secs(2));
            continue;
        }

        let matched_path = row["matchedPath"].as_str().unwrap_or(target_seg);
        let sub_score    = row["score"].as_u64().unwrap_or(0);

        if sub_score < 100 {
            emit_log(app, &format!(
                "  ⚠ Subcategory fuzzy match: '{}' → '{}' (score {})",
                target_seg, matched_path, sub_score
            ));
        }

        // ── Section header + match notices ───────────────────────────────────
        lines.push(format!("## {}", full_path));
        lines.push(String::new());

        let top_note = if top_score < 100 {
            format!("top-level matched to **{}**", top_matched_name)
        } else { String::new() };
        let sub_note = if sub_score < 100 {
            format!("subcategory matched to **{}**", matched_path)
        } else { String::new() };
        let notices: Vec<&str> = [top_note.as_str(), sub_note.as_str()]
            .iter().filter(|s| !s.is_empty()).copied().collect();
        if !notices.is_empty() {
            lines.push(format!("*⚠️ Closest match used — {}.*", notices.join(", ")));
            lines.push(String::new());
        }

        lines.push(format!("- **Sales to #1:** {}", row["salesToOne"].as_str().unwrap_or("")));
        lines.push(format!("- **Sales to #10:** {}", row["salesToTen"].as_str().unwrap_or("")));
        lines.push(format!("- **Publisher %:** {}", row["publisherPct"].as_str().unwrap_or("")));
        lines.push(format!("- **KU %:** {}", row["kuPct"].as_str().unwrap_or("")));
        lines.push(String::new());

        // ── Insights modal ───────────────────────────────────────────────────
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

        // ── Keywords modal ───────────────────────────────────────────────────
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
    let back_js = r#"
        const el = Array.from(document.querySelectorAll('button,a,span'))
          .find(e => e.textContent.trim() === 'Back' || e.textContent.trim() === '← Back');
        if (!el) return JSON.stringify(null);
        el.scrollIntoView({block:'center'});
        const r = el.getBoundingClientRect();
        return JSON.stringify({x:Math.round(r.x+r.width/2), y:Math.round(r.y+r.height/2)});
    "#;
    if let Ok(s) = session.eval(back_js, 8) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                let _ = session.click(x, y);
            }
        }
    }
}

/// PR sometimes hides the category search input behind a small icon button
/// until it's clicked. No-op if a text input is already visible.
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
        const aria = document.querySelector('[aria-label*="search" i]');
        if (aria) {
          const r = aria.getBoundingClientRect();
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


// ── Category Finder ───────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct FindRequest {
    pub genre: String,
    pub store: String,
    pub filter: String,
    pub api_key: String,
    pub model: String,
}

#[tauri::command]
pub async fn find_categories(
    app: AppHandle,
    request: FindRequest,
) -> AnalyzerResult {
    tokio::task::spawn_blocking(move || {
        use crate::category_finder;
        let now = chrono::Utc::now().format("%B %-d, %Y %H:%M UTC").to_string();

        match category_finder::find_categories(
            &app,
            &request.genre,
            &request.store,
            &request.filter,
            &request.api_key,
            &request.model,
        ) {
            Err(e) => AnalyzerResult { success: false, markdown: String::new(), error: e },
            Ok(results) => {
                let mut lines = vec![
                    "# Category Finder Results".to_string(),
                    format!("Genre: {}", request.genre),
                    format!("Generated: {}", now),
                    String::new(),
                ];

                // Check if any result has keywords (high-confidence matches)
                // Split results into high-confidence (≥80%) and low-confidence
                let high: Vec<_> = results.iter().filter(|r| r.confidence >= 80).collect();
                let low:  Vec<_> = results.iter().filter(|r| r.confidence <  80).collect();

                if !high.is_empty() {
                    lines.push("## Matched Categories".to_string());
                    lines.push(String::new());
                    for r in &high {
                        lines.push(format!("### {} ({}% match)", r.path, r.confidence));
                        lines.push(String::new());
                        if !r.stats.is_empty() {
                            lines.push("**Sales Potential**".to_string());
                            lines.push(String::new());
                            lines.push(format!("- Sales needed to reach #1: **{}**", r.stats.sales_to_one));
                            lines.push(format!("- Sales needed to reach #10: **{}**", r.stats.sales_to_ten));
                            lines.push(format!("- Publisher books: **{}**", r.stats.publisher_pct));
                            lines.push(format!("- KU books: **{}**", r.stats.ku_pct));
                            lines.push(String::new());
                        }
                        if !r.keywords.is_empty() {
                            lines.push("**Keywords**".to_string());
                            lines.push(String::new());
                            lines.push(r.keywords.clone());
                            lines.push(String::new());
                        } else {
                            lines.push("*⚠️ Keywords could not be scraped for this category.*".to_string());
                            lines.push(String::new());
                        }
                        lines.push("---".to_string());
                        lines.push(String::new());
                    }
                } else {
                    lines.push("## No High-Confidence Match Found".to_string());
                    lines.push(String::new());
                    lines.push("The following categories were found but none reached the 80% confidence threshold.".to_string());
                    lines.push("Use one of these paths in the **Category Analyzer** to get keywords.".to_string());
                    lines.push(String::new());
                    for (i, r) in results.iter().enumerate() {
                        lines.push(format!("{}. {} — **{}%**", i + 1, r.path, r.confidence));
                    }
                    lines.push(String::new());
                }

                // Always append any low-confidence candidates at the bottom
                if !low.is_empty() {
                    lines.push("## Also Considered (below 80%)".to_string());
                    lines.push(String::new());
                    for (i, r) in low.iter().enumerate() {
                        lines.push(format!("{}. {} — **{}%**", i + 1, r.path, r.confidence));
                    }
                    lines.push(String::new());
                }

                AnalyzerResult {
                    success: true,
                    markdown: lines.join("\n"),
                    error: String::new(),
                }
            }
        }
    }).await.unwrap()
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
    let analysis_model = models::resolve_analysis_model(&req.model);
    emit_log(app, &format!("Running CSV Analyzer for keyword: {} [{}]...", req.keyword, analysis_model));

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

    match call_anthropic(&req.api_key, analysis_model, system, &user, 1500) {
        Ok(markdown) => {
            emit_log(app, "✓ CSV analysis complete.");
            AnalyzerResult { success: true, markdown, error: String::new() }
        }
        Err(e) => AnalyzerResult { success: false, markdown: String::new(), error: e },
    }
}

// ── AI client ─────────────────────────────────────────────────────────────────

pub fn call_anthropic(
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
