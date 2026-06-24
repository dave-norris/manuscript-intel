// pr_scraper.rs — Direct Publisher Rocket CDP navigation
//
// Takes a known category path (e.g. from genre-data.json) and navigates
// Publisher Rocket to scrape stats and keywords without any AI involvement.
//
// The path navigation works by:
//   1. Searching for the top-level segment (first word before & or ,)
//   2. Clicking "Check it out" on the best-matching top-level row
//   3. Finding the subcategory row whose last segment matches target
//   4. Scraping stats + clicking Keywords

use std::time::Duration;
use serde_json::Value;
use tauri::{AppHandle, Emitter};

use crate::cdp;
use crate::category_finder::CategoryStats;

#[allow(dead_code)]
pub struct ScrapedCategory {
    pub path:     String,
    pub store:    String, // "Kindle" or "Books"
    pub stats:    CategoryStats,
    pub keywords: String,
}

pub fn scrape_category_paths(
    app: &AppHandle,
    paths: &[String],
    store: &str,       // "Kindle" or "Books"
    filter: &str,
) -> Vec<ScrapedCategory> {
    emit(app, &format!("Connecting to Publisher Rocket for {} scrape...", store));

    let target = match cdp::ensure_rocket() {
        Ok(t) => t,
        Err(e) => { emit(app, &format!("  ⚠ CDP error: {}", e)); return Vec::new(); }
    };
    let mut session = match cdp::connect(&target) {
        Ok(s) => s,
        Err(e) => { emit(app, &format!("  ⚠ CDP connect error: {}", e)); return Vec::new(); }
    };
    emit(app, "  CDP session established.");

    let mut results = Vec::new();

    for (i, path) in paths.iter().enumerate() {
        emit(app, &format!("  [{}/{}] {}", i + 1, paths.len(), path));

        // ── Navigate to Category Search ──────────────────────────────────────
        nav_category_search(&mut session);
        std::thread::sleep(Duration::from_secs(2));

        // ── Set store radio ──────────────────────────────────────────────────
        click_label(&mut session, store);
        std::thread::sleep(Duration::from_millis(800));

        // ── Set filter ───────────────────────────────────────────────────────
        if filter != "All" {
            click_exact_button(&mut session, filter);
            std::thread::sleep(Duration::from_millis(600));
        }

        // ── Parse path into segments ─────────────────────────────────────────
        let segments: Vec<&str> = path.split('>')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();

        if segments.is_empty() { continue; }

        // Strip "Kindle Books" / "Kindle Store" prefix if present
        let segments: Vec<&str> = if segments[0].to_lowercase().contains("kindle") ||
                                     segments[0].to_lowercase().contains("books store") {
            segments[1..].to_vec()
        } else {
            segments
        };

        if segments.is_empty() { continue; }

        let top = segments[0];

        // ── Search for top-level category ────────────────────────────────────
        type_into_search(&mut session, top);
        std::thread::sleep(Duration::from_secs(2));

        // ── Click best "Check it out" row ────────────────────────────────────
        if !click_best_check_it_out(&mut session, top) {
            emit(app, &format!("    ⚠ Top-level '{}' not found in PR", top));
            continue;
        }
        std::thread::sleep(Duration::from_secs(5));

        // ── Find matching subcategory row ────────────────────────────────────
        // Target is the last segment of the path
        let target_seg = segments[segments.len() - 1];
        let (stats, keywords) = scrape_matching_row(&mut session, target_seg);

        if stats.is_empty() && keywords.is_empty() {
            emit(app, &format!("    ⚠ Could not find '{}' in subcategory table", target_seg));
        } else {
            emit(app, &format!(
                "    ✓ Stats: #1={} #10={} Pub={}% KU={}% | Keywords: {} chars",
                stats.sales_to_one, stats.sales_to_ten,
                stats.publisher_pct, stats.ku_pct,
                keywords.len()
            ));
        }

        results.push(ScrapedCategory {
            path: path.clone(),
            store: store.to_string(),
            stats,
            keywords,
        });

        click_back(&mut session);
        std::thread::sleep(Duration::from_secs(2));
    }

    results
}

// ── CDP helpers ───────────────────────────────────────────────────────────────

fn emit(app: &AppHandle, msg: &str) { let _ = app.emit("genre:log", msg); }

fn nav_category_search(session: &mut cdp::Session) {
    let js = r#"
        const el = Array.from(document.querySelectorAll('p,span,div,a'))
          .find(e => e.children.length === 0 && e.textContent.trim() === 'Category Search');
        if (!el) return JSON.stringify(null);
        el.scrollIntoView({block:'center'});
        const r = el.getBoundingClientRect();
        return JSON.stringify({x:Math.round(r.x+r.width/2),y:Math.round(r.y+r.height/2)});
    "#;
    if let Ok(s) = session.eval(js, 8) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                let _ = session.click(x, y);
            }
        }
    }
}

fn click_label(session: &mut cdp::Session, text: &str) {
    let tj = serde_json::to_string(text).unwrap();
    let js = format!(r#"
        const el = Array.from(document.querySelectorAll('label,span,p,input'))
          .find(e => e.textContent && e.textContent.trim() === {t});
        if (!el) return JSON.stringify(null);
        const t2 = el.closest('label') || el;
        const r = t2.getBoundingClientRect();
        return JSON.stringify({{x:Math.round(r.x+r.width/2),y:Math.round(r.y+r.height/2)}});
    "#, t = tj);
    if let Ok(s) = session.eval(&js, 8) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                let _ = session.click(x, y);
            }
        }
    }
}

fn click_exact_button(session: &mut cdp::Session, text: &str) {
    let tj = serde_json::to_string(text).unwrap();
    let js = format!(r#"
        const btn = Array.from(document.querySelectorAll('button,span,div'))
          .find(e => e.children.length === 0 && e.textContent.trim() === {t});
        if (!btn) return JSON.stringify(null);
        btn.scrollIntoView({{block:'center'}});
        const r = btn.getBoundingClientRect();
        return JSON.stringify({{x:Math.round(r.x+r.width/2),y:Math.round(r.y+r.height/2)}});
    "#, t = tj);
    if let Ok(s) = session.eval(&js, 8) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                let _ = session.click(x, y);
            }
        }
    }
}

fn type_into_search(session: &mut cdp::Session, category: &str) {
    // Use first word before & or , to avoid React input issues with special chars
    let search_term = category.split(|c| c == '&' || c == ',')
        .next().unwrap_or(category).trim().to_string();
    let sj = serde_json::to_string(&search_term).unwrap();

    let js = format!(r#"
        const input = document.querySelector(
            'input[type="text"],input[type="search"],input:not([type="radio"]):not([type="checkbox"])'
        );
        if (input) {{
            input.focus();
            input.value = '';
            input.dispatchEvent(new Event('input',{{bubbles:true}}));
            const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype,'value').set;
            setter.call(input, {s});
            input.dispatchEvent(new Event('input',{{bubbles:true}}));
            input.dispatchEvent(new Event('change',{{bubbles:true}}));
        }}
        return '';
    "#, s = sj);
    let _ = session.eval(&js, 8);
}

fn click_best_check_it_out(session: &mut cdp::Session, category: &str) -> bool {
    let cj = serde_json::to_string(category).unwrap();
    let js = format!(r#"
        const wanted = {c}.toLowerCase();
        const rows = Array.from(document.querySelectorAll('tr'));
        let bestBtn = null, bestScore = 0;
        for (const row of rows) {{
            const cells = row.querySelectorAll('td');
            if (!cells.length) continue;
            const name = cells[0].textContent.trim().toLowerCase();
            const btn = Array.from(row.querySelectorAll('button'))
                .find(b => b.textContent.trim() === 'Check it out');
            if (!btn) continue;
            let score = 0;
            if (name === wanted)                                          score = 100;
            else if (name.startsWith(wanted) || wanted.startsWith(name)) score = 80;
            else if (name.includes(wanted)   || wanted.includes(name))   score = 60;
            if (score > bestScore) {{ bestScore = score; bestBtn = btn; }}
        }}
        if (!bestBtn) return JSON.stringify(null);
        bestBtn.scrollIntoView({{block:'center'}});
        const r = bestBtn.getBoundingClientRect();
        return JSON.stringify({{x:Math.round(r.x+r.width/2),y:Math.round(r.y+r.height/2)}});
    "#, c = cj);

    if let Ok(s) = session.eval(&js, 8) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                let _ = session.click(x, y);
                return true;
            }
        }
    }
    false
}

/// Find the subcategory row whose last path segment best matches target_seg,
/// scrape its stats, and click Keywords.
fn scrape_matching_row(session: &mut cdp::Session, target_seg: &str) -> (CategoryStats, String) {
    let tj = serde_json::to_string(target_seg).unwrap();

    let row_js = format!(r#"
        const target = {t}.toLowerCase().trim();
        const dataRows = Array.from(document.querySelectorAll('tr'))
            .slice(1)
            .filter(r => r.querySelectorAll('td').length >= 2);

        let bestRow = null, bestScore = 0, bestIdx = -1;
        dataRows.forEach((row, idx) => {{
            const cells = Array.from(row.querySelectorAll('td'));
            const txt = (cells[0]?.textContent || '').replace(/\s+/g,' ').trim();
            const lastSeg = txt.split('>').pop().trim().toLowerCase();
            let score = 0;
            if (lastSeg === target)                                             score = 100;
            else if (lastSeg.startsWith(target) || target.startsWith(lastSeg)) score = 80;
            else if (lastSeg.includes(target)   || target.includes(lastSeg))   score = 60;
            else {{
                const s = target.length < lastSeg.length ? target : lastSeg;
                const l = target.length < lastSeg.length ? lastSeg : target;
                let ov = 0;
                for (let i = 0; i < s.length; i++) {{ if (l.includes(s[i])) ov++; }}
                score = Math.round((ov / l.length) * 40);
            }}
            if (score > bestScore) {{ bestScore = score; bestRow = row; bestIdx = idx; }}
        }});

        if (!bestRow) return JSON.stringify(null);
        const cells = Array.from(bestRow.querySelectorAll('td'));
        const btns  = Array.from(bestRow.querySelectorAll('button'));
        const kBtn  = btns.find(b => b.textContent.trim() === 'Keywords');
        const rk    = kBtn ? kBtn.getBoundingClientRect() : null;
        return JSON.stringify({{
            score:        bestScore,
            salesToOne:   cells[1]?.textContent.trim() ?? '',
            salesToTen:   cells[2]?.textContent.trim() ?? '',
            publisherPct: cells[3]?.textContent.trim() ?? '',
            kuPct:        cells[4]?.textContent.trim() ?? '',
            kCoords: rk ? {{x:Math.round(rk.x+rk.width/2),y:Math.round(rk.y+rk.height/2)}} : null,
        }});
    "#, t = tj);

    let val: Value = match session.eval(&row_js, 15) {
        Ok(ref s) if s != "null" && !s.is_empty() => {
            serde_json::from_str(s).unwrap_or(Value::Null)
        }
        _ => Value::Null,
    };

    if val.is_null() { return (CategoryStats::empty(), String::new()); }

    let stats = CategoryStats {
        sales_to_one:  val["salesToOne"].as_str().unwrap_or("").to_string(),
        sales_to_ten:  val["salesToTen"].as_str().unwrap_or("").to_string(),
        publisher_pct: val["publisherPct"].as_str().unwrap_or("").to_string(),
        ku_pct:        val["kuPct"].as_str().unwrap_or("").to_string(),
    };

    let keywords = match (val["kCoords"]["x"].as_f64(), val["kCoords"]["y"].as_f64()) {
        (Some(x), Some(y)) => {
            let _ = session.click(x, y);
            std::thread::sleep(Duration::from_secs(2));
            let modal_js = r#"
                const o = document.querySelector('[class*="modal"],[class*="overlay"],[class*="popup"]');
                return o ? o.innerText : '';
            "#;
            let kw = session.eval(modal_js, 8).unwrap_or_default();
            session.key_escape();
            std::thread::sleep(Duration::from_millis(500));
            kw
        }
        _ => String::new(),
    };

    (stats, keywords)
}

fn click_back(session: &mut cdp::Session) {
    let js = r#"
        const el = Array.from(document.querySelectorAll('button,a,span'))
          .find(e => e.textContent.trim() === 'Back' || e.textContent.trim() === '← Back');
        if (!el) return JSON.stringify(null);
        el.scrollIntoView({block:'center'});
        const r = el.getBoundingClientRect();
        return JSON.stringify({x:Math.round(r.x+r.width/2),y:Math.round(r.y+r.height/2)});
    "#;
    if let Ok(s) = session.eval(js, 8) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                let _ = session.click(x, y);
            }
        }
    }
}
