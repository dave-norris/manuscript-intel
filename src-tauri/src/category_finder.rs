// category_finder.rs — AI-driven category finder

use std::time::Duration;
use serde::Deserialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter};

use crate::cdp;
use crate::commands::call_anthropic;
use crate::models;

// ── Stored top-level Kindle categories ───────────────────────────────────────

pub const TOP_LEVEL_CATEGORIES: &[&str] = &[
    "Literature & Fiction",
    "Mystery, Thriller & Suspense",
    "Science Fiction & Fantasy",
    "Romance",
    "Religion & Spirituality",
    "Teen & Young Adult",
    "Children's eBooks",
    "Biographies & Memoirs",
    "History",
    "Self-Help",
    "Health, Fitness & Dieting",
    "Business & Money",
    "Computers & Technology",
    "Comics, Manga & Graphic Novels",
    "Humor & Entertainment",
    "Arts & Photography",
    "Crafts, Hobbies & Home",
    "Travel",
    "Education & Teaching",
    "LGBTQ+ eBooks",
    "Classics",
];

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CategoryStats {
    pub sales_to_one:  String,
    pub sales_to_ten:  String,
    pub publisher_pct: String,
    pub ku_pct:        String,
}

impl CategoryStats {
    pub fn empty() -> Self {
        Self {
            sales_to_one:  String::new(),
            sales_to_ten:  String::new(),
            publisher_pct: String::new(),
            ku_pct:        String::new(),
        }
    }
    pub fn is_empty(&self) -> bool { self.sales_to_one.is_empty() }
}

#[derive(Debug, Clone)]
pub struct ScoredCategory {
    pub path:       String,
    pub confidence: u8,
    pub stats:      CategoryStats,
    pub keywords:   String,
}

#[derive(Debug, Deserialize)]
struct TopLevelRanking {
    category:   String,
    confidence: u8,
    reason:     String,
}

// AI returns the 1-based index into the subcategory row list plus confidence
#[derive(Debug, Deserialize)]
struct SubcategoryMatch {
    index:      usize,   // 1-based index into the rows list
    path:       String,  // copied verbatim for display only
    confidence: u8,
    reason:     String,
}

// ── Main entry point ──────────────────────────────────────────────────────────

pub fn find_categories(
    app: &AppHandle,
    genre_description: &str,
    store: &str,
    filter: &str,
    api_key: &str,
    _model: &str,  // reserved — category matching always uses Haiku
) -> Result<Vec<ScoredCategory>, String> {

    emit(app, "Step 1: Asking AI to rank top-level categories...");
    let top_rankings = ai_rank_top_level(genre_description, api_key, models::HAIKU)?;

    emit(app, &format!("  {} top-level candidates ranked.", top_rankings.len()));
    for r in &top_rankings {
        emit(app, &format!("  {}% — {}: {}", r.confidence, r.category, r.reason));
    }

    let mut all_candidates: Vec<ScoredCategory> = Vec::new();
    let mut results:        Vec<ScoredCategory> = Vec::new();

    let high_confidence: Vec<&TopLevelRanking> = top_rankings.iter()
        .filter(|r| r.confidence >= 80)
        .collect();

    if high_confidence.is_empty() {
        emit(app, "  No top-level category reached 80% confidence.");
        for r in &top_rankings {
            all_candidates.push(ScoredCategory {
                path: r.category.clone(), confidence: r.confidence,
                stats: CategoryStats::empty(), keywords: String::new(),
            });
        }
        all_candidates.sort_by(|a, b| b.confidence.cmp(&a.confidence));
        return Ok(all_candidates);
    }

    emit(app, "Step 2: Connecting to Publisher Rocket...");
    let target = cdp::ensure_rocket()?;
    let mut session = cdp::connect(&target)?;
    emit(app, "  CDP session established.");

    for top in &high_confidence {
        emit(app, &format!("Step 3: '{}' ({}%) — navigating...", top.category, top.confidence));

        navigate_to_category_search(&mut session)?;
        std::thread::sleep(Duration::from_secs(2));

        click_radio(&mut session, store);
        std::thread::sleep(Duration::from_millis(800));

        if filter != "All" {
            click_button_by_text(&mut session, filter);
            std::thread::sleep(Duration::from_millis(600));
        }

        if !click_check_it_out(&mut session, &top.category) {
            emit(app, &format!("  ⚠ '{}' not found in Publisher Rocket — skipping.", top.category));
            all_candidates.push(ScoredCategory {
                path: top.category.clone(), confidence: top.confidence,
                stats: CategoryStats::empty(), keywords: String::new(),
            });
            continue;
        }
        std::thread::sleep(Duration::from_secs(5));

        // Scrape rows — keep them in order so the AI index is stable
        let rows = scrape_subcategory_rows(&mut session);
        emit(app, &format!("  {} subcategory rows visible.", rows.len()));

        if rows.is_empty() {
            emit(app, "  ⚠ No subcategory rows found.");
            click_back(&mut session);
            std::thread::sleep(Duration::from_secs(2));
            all_candidates.push(ScoredCategory {
                path: top.category.clone(), confidence: top.confidence,
                stats: CategoryStats::empty(), keywords: String::new(),
            });
            continue;
        }

        // Log first few rows so we can see what was scraped
        for (i, row) in rows.iter().take(5).enumerate() {
            emit(app, &format!("  Row {}: {}", i + 1, row));
        }

        emit(app, "  Asking AI to pick best subcategories by index (up to 3)...");
        match ai_match_subcategories(genre_description, &top.category, &rows, api_key, models::HAIKU) {
            Err(e) => {
                emit(app, &format!("  ⚠ AI match failed: {}", e));
                click_back(&mut session);
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
            Ok(matches) => {
                // De-dupe by row index within this single top-level branch —
                // the AI can occasionally repeat an index across list items.
                let mut seen_indices = std::collections::HashSet::new();

                for m in matches {
                    // Clamp index to valid range
                    let row_index = if m.index == 0 { 1 } else { m.index }.min(rows.len());
                    if !seen_indices.insert(row_index) { continue; }

                    let display_path = rows.get(row_index - 1)
                        .cloned()
                        .unwrap_or_else(|| m.path.clone());

                    emit(app, &format!("  {}% — row {} '{}': {}", m.confidence, row_index, display_path, m.reason));

                    if m.confidence >= 80 {
                        emit(app, &format!("  ✓ Scraping stats + keywords for row {}...", row_index));

                        // Use the 0-based DOM index to find the row — no string matching
                        let (stats, keywords) = scrape_by_row_index(&mut session, row_index - 1);

                        emit(app, &format!(
                            "  Stats: #1={} #10={} Publisher={}% KU={}%",
                            stats.sales_to_one, stats.sales_to_ten,
                            stats.publisher_pct, stats.ku_pct
                        ));
                        emit(app, &format!("  Keywords: {} chars", keywords.len()));

                        results.push(ScoredCategory {
                            path: display_path,
                            confidence: m.confidence,
                            stats,
                            keywords,
                        });
                    } else {
                        emit(app, "  Below 80% — adding to candidates.");
                        all_candidates.push(ScoredCategory {
                            path: display_path, confidence: m.confidence,
                            stats: CategoryStats::empty(), keywords: String::new(),
                        });
                    }
                }

                click_back(&mut session);
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    }

    for r in top_rankings.iter().filter(|r| r.confidence < 80) {
        all_candidates.push(ScoredCategory {
            path: r.category.clone(), confidence: r.confidence,
            stats: CategoryStats::empty(), keywords: String::new(),
        });
    }

    if results.is_empty() {
        emit(app, "No high-confidence matches. Returning ranked candidates.");
        all_candidates.sort_by(|a, b| b.confidence.cmp(&a.confidence));
        Ok(all_candidates)
    } else {
        // De-dupe across top-level branches — the same subcategory can surface
        // under more than one top-level search (e.g. a Christian Fiction row
        // appearing under both "Religion & Spirituality" and "Mystery, Thriller
        // & Suspense"). Keep the highest-confidence copy of each path.
        let mut by_path: std::collections::HashMap<String, ScoredCategory> = std::collections::HashMap::new();
        for r in results {
            let key = r.path.to_lowercase();
            match by_path.get(&key) {
                Some(existing) if existing.confidence >= r.confidence => {}
                _ => { by_path.insert(key, r); }
            }
        }
        let mut deduped: Vec<ScoredCategory> = by_path.into_values().collect();
        deduped.sort_by(|a, b| b.confidence.cmp(&a.confidence));

        // Merge in any below-threshold candidates too
        deduped.extend(all_candidates);
        emit(app, &format!("✓ Done — {} result(s) after de-duplication.", deduped.len()));
        Ok(deduped)
    }
}

// ── AI calls ──────────────────────────────────────────────────────────────────

fn ai_rank_top_level(genre: &str, api_key: &str, model: &str)
    -> Result<Vec<TopLevelRanking>, String>
{
    let system = r#"You are an Amazon Kindle publishing expert. Rank the provided top-level Kindle categories by how well they match the genre description.

Return ONLY a JSON array, no markdown, no preamble.
Each item: { "category": "<exact name from list>", "confidence": <0-100>, "reason": "<one sentence>" }
Only include items with confidence > 20. Sort descending by confidence."#;

    let user = format!(
        "Genre: {}\n\nCategories:\n{}",
        genre,
        TOP_LEVEL_CATEGORIES.join("\n")
    );

    let raw = call_anthropic(api_key, model, system, &user, 800)?;
    let clean = raw.trim()
        .trim_start_matches("```json").trim_start_matches("```")
        .trim_end_matches("```").trim();

    serde_json::from_str::<Vec<TopLevelRanking>>(clean)
        .map_err(|e| format!("Parse error (top-level): {} | got: {}", e, &clean[..clean.len().min(300)]))
}

fn ai_match_subcategories(
    genre: &str,
    top_category: &str,
    rows: &[String],
    api_key: &str,
    model: &str,
) -> Result<Vec<SubcategoryMatch>, String> {
    let numbered = rows.iter()
        .enumerate()
        .map(|(i, r)| format!("{}. {}", i + 1, r))
        .collect::<Vec<_>>()
        .join("\n");

    let system = r#"You are an Amazon Kindle publishing expert. This book is cross-genre — pick EVERY subcategory in this list that is a genuinely strong, distinct fit, not just the single closest match.

Return ONLY a JSON array, no markdown, no preamble.
Return 1 to 3 items. Only return more than one if they are meaningfully different fits — e.g. one subcategory serving the book's mystery/suspense thread and a separate one serving its historical or literary thread. Do not pad the list with near-duplicates of your top pick.
Each item: { "index": <1-based row number>, "path": "<copied verbatim>", "confidence": <0-100>, "reason": "<one sentence>" }
The index must match the number at the start of the row you choose. Sort descending by confidence."#;

    let user = format!(
        "Genre: {}\nTop-level category: {}\n\nSubcategory rows:\n{}",
        genre, top_category, numbered
    );

    let raw = call_anthropic(api_key, model, system, &user, 500)?;
    let clean = raw.trim()
        .trim_start_matches("```json").trim_start_matches("```")
        .trim_end_matches("```").trim();

    serde_json::from_str::<Vec<SubcategoryMatch>>(clean)
        .map_err(|e| format!("Parse error (subcategory): {} | got: {}", e, &clean[..clean.len().min(300)]))
}

// ── CDP helpers ───────────────────────────────────────────────────────────────

fn emit(app: &AppHandle, msg: &str) { let _ = app.emit("cdp:log", msg); }

fn navigate_to_category_search(session: &mut cdp::Session) -> Result<(), String> {
    let js = r#"
        const el = Array.from(document.querySelectorAll('p,span,div,a'))
          .find(e => e.children.length === 0 && e.textContent.trim() === 'Category Search');
        if (!el) return JSON.stringify(null);
        el.scrollIntoView({block:'center'});
        const r = el.getBoundingClientRect();
        return JSON.stringify({x:Math.round(r.x+r.width/2), y:Math.round(r.y+r.height/2)});
    "#;
    if let Ok(s) = session.eval(js, 8) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                let _ = session.click(x, y);
            }
        }
    }
    Ok(())
}

fn click_radio(session: &mut cdp::Session, label: &str) {
    let lj = serde_json::to_string(label).unwrap();
    let js = format!(r#"
        const el = Array.from(document.querySelectorAll('label,span,p,input'))
          .find(e => e.textContent && e.textContent.trim() === {l});
        if (!el) return JSON.stringify(null);
        const t = el.closest('label') || el;
        const r = t.getBoundingClientRect();
        return JSON.stringify({{x:Math.round(r.x+r.width/2),y:Math.round(r.y+r.height/2)}});
    "#, l = lj);
    if let Ok(s) = session.eval(&js, 8) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                let _ = session.click(x, y);
            }
        }
    }
}

fn click_button_by_text(session: &mut cdp::Session, text: &str) {
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

fn click_check_it_out(session: &mut cdp::Session, category: &str) -> bool {
    let search_term = category.split(|c| c == '&' || c == ',')
        .next().unwrap_or(category).trim().to_string();
    let sj = serde_json::to_string(&search_term).unwrap();
    let cj = serde_json::to_string(category).unwrap();

    // PR sometimes hides the search input behind an icon until it's clicked.
    click_search_icon_if_present(session);

    let type_js = format!(r#"
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
    let _ = session.eval(&type_js, 8);
    std::thread::sleep(Duration::from_secs(2));

    let cio_js = format!(r#"
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

    if let Ok(s) = session.eval(&cio_js, 8) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                let _ = session.click(x, y);
                return true;
            }
        }
    }
    false
}

/// Collect subcategory rows in DOM order — the index returned by the AI maps
/// directly to position in this Vec (0-based).
fn scrape_subcategory_rows(session: &mut cdp::Session) -> Vec<String> {
    let js = r#"
        const rows = Array.from(document.querySelectorAll('tr')).slice(1);
        const out = [];
        for (const row of rows) {
            const cells = Array.from(row.querySelectorAll('td'));
            if (cells.length < 2) continue;
            const txt = (cells[0]?.textContent || '').replace(/\s+/g,' ').trim();
            if (txt) out.push(txt);
        }
        return JSON.stringify(out);
    "#;
    if let Ok(s) = session.eval(js, 15) {
        if let Ok(v) = serde_json::from_str::<Vec<String>>(&s) {
            return v;
        }
    }
    Vec::new()
}

/// Locate a data row by its 0-based index in the table, scrape stats and click
/// Keywords — no string matching involved at all.
fn scrape_by_row_index(session: &mut cdp::Session, index: usize) -> (CategoryStats, String) {
    let idx = index as u64;
    let row_js = format!(r#"
        // Collect only rows that have at least 2 td cells (data rows, not header)
        const dataRows = Array.from(document.querySelectorAll('tr'))
            .slice(1)
            .filter(r => r.querySelectorAll('td').length >= 2);
        const row = dataRows[{idx}];
        if (!row) return JSON.stringify(null);
        const cells = Array.from(row.querySelectorAll('td'));
        const btns  = Array.from(row.querySelectorAll('button'));
        const kBtn  = btns.find(b => b.textContent.trim() === 'Keywords');
        const rk    = kBtn ? kBtn.getBoundingClientRect() : null;
        return JSON.stringify({{
            salesToOne:   cells[1]?.textContent.trim() ?? '',
            salesToTen:   cells[2]?.textContent.trim() ?? '',
            publisherPct: cells[3]?.textContent.trim() ?? '',
            kuPct:        cells[4]?.textContent.trim() ?? '',
            kCoords: rk ? {{x:Math.round(rk.x+rk.width/2),y:Math.round(rk.y+rk.height/2)}} : null,
        }});
    "#, idx = idx);

    let val: Value = match session.eval(&row_js, 10) {
        Ok(ref s) if s != "null" && !s.is_empty() => {
            serde_json::from_str(s).unwrap_or(Value::Null)
        }
        _ => Value::Null,
    };

    if val.is_null() {
        return (CategoryStats::empty(), String::new());
    }

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
