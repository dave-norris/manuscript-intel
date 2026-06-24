// category_finder.rs — AI-driven category finder
//
// Flow:
//   1. AI ranks stored top-level categories by confidence against genre description
//   2. For each top-level ≥ 80%: click "Check it out", scrape subcategory rows
//   3. AI picks best subcategory match with confidence
//   4. If ≥ 80%: scrape stats + keywords → commit result
//   5. If nothing reaches 80% anywhere: return all candidates ranked high→low

use std::time::Duration;
use serde::Deserialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter};

use crate::cdp;
use crate::commands::call_anthropic;

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
    pub sales_to_one: String,
    pub sales_to_ten: String,
    pub publisher_pct: String,
    pub ku_pct: String,
}

impl CategoryStats {
    pub fn empty() -> Self {
        Self {
            sales_to_one: String::new(),
            sales_to_ten: String::new(),
            publisher_pct: String::new(),
            ku_pct: String::new(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.sales_to_one.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct ScoredCategory {
    pub path: String,
    pub confidence: u8,
    pub stats: CategoryStats,
    pub keywords: String,
}

#[derive(Debug, Deserialize)]
struct TopLevelRanking {
    category: String,
    confidence: u8,
    reason: String,
}

#[derive(Debug, Deserialize)]
struct SubcategoryMatch {
    path: String,
    confidence: u8,
    reason: String,
}

// ── Main entry point ──────────────────────────────────────────────────────────

pub fn find_categories(
    app: &AppHandle,
    genre_description: &str,
    store: &str,
    filter: &str,
    api_key: &str,
    model: &str,
) -> Result<Vec<ScoredCategory>, String> {

    emit(app, "Step 1: Asking AI to rank top-level categories...");

    let top_rankings = ai_rank_top_level(genre_description, api_key, model)?;

    emit(app, &format!("  {} top-level candidates ranked.", top_rankings.len()));
    for r in &top_rankings {
        emit(app, &format!("  {}% — {}: {}", r.confidence, r.category, r.reason));
    }

    let mut all_candidates: Vec<ScoredCategory> = Vec::new();
    let mut results: Vec<ScoredCategory> = Vec::new();

    let high_confidence: Vec<&TopLevelRanking> = top_rankings.iter()
        .filter(|r| r.confidence >= 80)
        .collect();

    if high_confidence.is_empty() {
        emit(app, "  No top-level category reached 80% confidence.");
        for r in &top_rankings {
            all_candidates.push(ScoredCategory {
                path: r.category.clone(),
                confidence: r.confidence,
                stats: CategoryStats::empty(),
                keywords: String::new(),
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
        emit(app, &format!("Step 3: Navigating to '{}' ({}%)...", top.category, top.confidence));

        navigate_to_category_search(&mut session)?;
        std::thread::sleep(Duration::from_secs(2));

        click_radio(&mut session, store);
        std::thread::sleep(Duration::from_millis(800));

        if filter != "All" {
            click_button_by_text(&mut session, filter);
            std::thread::sleep(Duration::from_millis(600));
        }

        let clicked = click_check_it_out(&mut session, &top.category);
        if !clicked {
            emit(app, &format!("  ⚠ Could not find '{}' in Publisher Rocket — skipping.", top.category));
            all_candidates.push(ScoredCategory {
                path: top.category.clone(),
                confidence: top.confidence,
                stats: CategoryStats::empty(),
                keywords: String::new(),
            });
            continue;
        }
        std::thread::sleep(Duration::from_secs(5));

        let subcategory_rows = scrape_subcategory_rows(&mut session);
        emit(app, &format!("  Scraped {} subcategory rows.", subcategory_rows.len()));

        if subcategory_rows.is_empty() {
            emit(app, "  ⚠ No subcategory rows found.");
            click_back(&mut session);
            std::thread::sleep(Duration::from_secs(2));
            all_candidates.push(ScoredCategory {
                path: top.category.clone(),
                confidence: top.confidence,
                stats: CategoryStats::empty(),
                keywords: String::new(),
            });
            continue;
        }

        emit(app, "  Asking AI to match subcategory...");
        let sub_match = ai_match_subcategory(
            genre_description, &top.category, &subcategory_rows, api_key, model
        );

        match sub_match {
            Err(e) => {
                emit(app, &format!("  ⚠ AI subcategory match failed: {}", e));
                click_back(&mut session);
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
            Ok(m) => {
                emit(app, &format!("  {}% — {}: {}", m.confidence, m.path, m.reason));

                if m.confidence >= 80 {
                    emit(app, &format!("  ✓ High confidence. Scraping stats + keywords for '{}'...", m.path));

                    let (stats, keywords) = scrape_stats_and_keywords(&mut session, &m.path);
                    emit(app, &format!(
                        "  Stats: #{} to sell to #1, #{} to #10 | Publisher {}% | KU {}%",
                        stats.sales_to_one, stats.sales_to_ten,
                        stats.publisher_pct, stats.ku_pct
                    ));
                    emit(app, &format!("  Keywords scraped ({} chars).", keywords.len()));

                    results.push(ScoredCategory {
                        path: m.path.clone(),
                        confidence: m.confidence,
                        stats,
                        keywords,
                    });
                } else {
                    emit(app, "  Below 80% — adding to candidates list.");
                    all_candidates.push(ScoredCategory {
                        path: m.path.clone(),
                        confidence: m.confidence,
                        stats: CategoryStats::empty(),
                        keywords: String::new(),
                    });
                }

                click_back(&mut session);
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    }

    for r in top_rankings.iter().filter(|r| r.confidence < 80) {
        all_candidates.push(ScoredCategory {
            path: r.category.clone(),
            confidence: r.confidence,
            stats: CategoryStats::empty(),
            keywords: String::new(),
        });
    }

    if results.is_empty() {
        emit(app, "No high-confidence matches found. Returning ranked candidates.");
        all_candidates.sort_by(|a, b| b.confidence.cmp(&a.confidence));
        Ok(all_candidates)
    } else {
        emit(app, &format!("✓ Found {} high-confidence match(es).", results.len()));
        Ok(results)
    }
}

// ── AI calls ──────────────────────────────────────────────────────────────────

fn ai_rank_top_level(
    genre: &str,
    api_key: &str,
    model: &str,
) -> Result<Vec<TopLevelRanking>, String> {
    let categories_list = TOP_LEVEL_CATEGORIES.join("\n");

    let system = r#"You are an Amazon Kindle publishing expert. You rank top-level Kindle Store categories by how well they match a given book genre description.

Return ONLY a JSON array. No preamble, no markdown fences, no explanation.

Each element must have exactly these fields:
- "category": string (must match exactly from the provided list)
- "confidence": integer 0-100 (how well this category fits the genre)
- "reason": string (one short sentence explaining the score)

Only include categories with confidence > 20. Sort by confidence descending."#;

    let user = format!(
        "Genre description: {}\n\nTop-level Kindle categories to rank:\n{}",
        genre, categories_list
    );

    let response = call_anthropic(api_key, model, system, &user, 800)?;
    let clean = response.trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    serde_json::from_str::<Vec<TopLevelRanking>>(clean)
        .map_err(|e| format!("Failed to parse top-level rankings: {} — response: {}", e, &clean[..clean.len().min(200)]))
}

fn ai_match_subcategory(
    genre: &str,
    top_category: &str,
    rows: &[String],
    api_key: &str,
    model: &str,
) -> Result<SubcategoryMatch, String> {
    let rows_list = rows.iter()
        .enumerate()
        .map(|(i, r)| format!("{}. {}", i + 1, r))
        .collect::<Vec<_>>()
        .join("\n");

    let system = r#"You are an Amazon Kindle publishing expert. You pick the single best matching subcategory path for a given book genre description.

Return ONLY a JSON object. No preamble, no markdown fences, no explanation.

The object must have exactly these fields:
- "path": string (must be copied verbatim from the provided list)
- "confidence": integer 0-100 (how well this subcategory fits the genre)
- "reason": string (one short sentence explaining the choice)"#;

    let user = format!(
        "Genre description: {}\n\nTop-level category: {}\n\nAvailable subcategory paths (pick the single best one):\n{}",
        genre, top_category, rows_list
    );

    let response = call_anthropic(api_key, model, system, &user, 400)?;
    let clean = response.trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    serde_json::from_str::<SubcategoryMatch>(clean)
        .map_err(|e| format!("Failed to parse subcategory match: {} — response: {}", e, &clean[..clean.len().min(200)]))
}

// ── CDP helpers ───────────────────────────────────────────────────────────────

fn emit(app: &AppHandle, msg: &str) {
    let _ = app.emit("cdp:log", msg);
}

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
    let label_json = serde_json::to_string(label).unwrap();
    let js = format!(r#"
        const el = Array.from(document.querySelectorAll('label,span,p,input'))
          .find(e => e.textContent && e.textContent.trim() === {l});
        if (!el) return JSON.stringify(null);
        const t = el.closest('label') || el;
        const r = t.getBoundingClientRect();
        return JSON.stringify({{x:Math.round(r.x+r.width/2), y:Math.round(r.y+r.height/2)}});
    "#, l = label_json);
    if let Ok(s) = session.eval(&js, 8) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                let _ = session.click(x, y);
            }
        }
    }
}

fn click_button_by_text(session: &mut cdp::Session, text: &str) {
    let text_json = serde_json::to_string(text).unwrap();
    let js = format!(r#"
        const btn = Array.from(document.querySelectorAll('button,span,div'))
          .find(e => e.children.length === 0 && e.textContent.trim() === {t});
        if (!btn) return JSON.stringify(null);
        btn.scrollIntoView({{block:'center'}});
        const r = btn.getBoundingClientRect();
        return JSON.stringify({{x:Math.round(r.x+r.width/2), y:Math.round(r.y+r.height/2)}});
    "#, t = text_json);
    if let Ok(s) = session.eval(&js, 8) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                let _ = session.click(x, y);
            }
        }
    }
}

fn click_check_it_out(session: &mut cdp::Session, category: &str) -> bool {
    let search_term = category
        .split(|c| c == '&' || c == ',')
        .next()
        .unwrap_or(category)
        .trim()
        .to_string();
    let search_json = serde_json::to_string(&search_term).unwrap();
    let cat_json    = serde_json::to_string(category).unwrap();

    let type_js = format!(r#"
        const input = document.querySelector(
            'input[type="text"],input[type="search"],input:not([type="radio"]):not([type="checkbox"])'
        );
        if (input) {{
            input.focus();
            input.value = '';
            input.dispatchEvent(new Event('input', {{bubbles:true}}));
            const setter = Object.getOwnPropertyDescriptor(
                window.HTMLInputElement.prototype, 'value'
            ).set;
            setter.call(input, {search});
            input.dispatchEvent(new Event('input', {{bubbles:true}}));
            input.dispatchEvent(new Event('change', {{bubbles:true}}));
        }}
        return '';
    "#, search = search_json);
    let _ = session.eval(&type_js, 8);
    std::thread::sleep(Duration::from_secs(2));

    let cio_js = format!(r#"
        const wanted = {cat}.toLowerCase();
        const rows = Array.from(document.querySelectorAll('tr'));
        let bestBtn = null, bestScore = 0;
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
            if (score > bestScore) {{ bestScore = score; bestBtn = btn; }}
        }}
        if (!bestBtn) return JSON.stringify(null);
        bestBtn.scrollIntoView({{block:'center'}});
        const r = bestBtn.getBoundingClientRect();
        return JSON.stringify({{x:Math.round(r.x+r.width/2), y:Math.round(r.y+r.height/2)}});
    "#, cat = cat_json);

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

fn scrape_subcategory_rows(session: &mut cdp::Session) -> Vec<String> {
    let js = r#"
        const rows = Array.from(document.querySelectorAll('tr')).slice(1);
        const paths = [];
        for (const row of rows) {
            const cells = Array.from(row.querySelectorAll('td'));
            if (cells.length < 2) continue;
            const txt = cells[0] ? cells[0].textContent.replace(/\s+/g,' ').trim() : '';
            if (txt.includes('>')) paths.push(txt);
        }
        return JSON.stringify(paths);
    "#;
    if let Ok(s) = session.eval(js, 15) {
        if let Ok(v) = serde_json::from_str::<Vec<String>>(&s) {
            return v;
        }
    }
    Vec::new()
}

/// Scrape both stats (sales figures, percentages) and keywords for a matched
/// subcategory path — all in one row lookup to avoid scanning the table twice.
fn scrape_stats_and_keywords(
    session: &mut cdp::Session,
    target_path: &str,
) -> (CategoryStats, String) {
    let target_json = serde_json::to_string(target_path).unwrap();

    // Find the row, pull stats, return coords for both Insights (unused here) and Keywords buttons
    let row_js = format!(r#"
        const target = {t}.toLowerCase()
            .replace(/^kindle books > /i, '')
            .replace(/^kindle store > /i, '');
        const rows = Array.from(document.querySelectorAll('tr')).slice(1);
        for (const row of rows) {{
            const cells = Array.from(row.querySelectorAll('td'));
            if (cells.length < 2) continue;
            const txt = (cells[0]?.textContent || '').replace(/\s+/g,' ').trim()
                .replace(/^Kindle Books > /i, '')
                .replace(/^Kindle Store > /i, '');
            if (txt.toLowerCase() !== target) continue;
            const btns = Array.from(row.querySelectorAll('button'));
            const kBtn = btns.find(b => b.textContent.trim() === 'Keywords');
            const rk = kBtn ? kBtn.getBoundingClientRect() : null;
            return JSON.stringify({{
                salesToOne:   cells[1]?.textContent.trim() ?? '',
                salesToTen:   cells[2]?.textContent.trim() ?? '',
                publisherPct: cells[3]?.textContent.trim() ?? '',
                kuPct:        cells[4]?.textContent.trim() ?? '',
                kCoords: rk ? {{x:Math.round(rk.x+rk.width/2), y:Math.round(rk.y+rk.height/2)}} : null,
            }});
        }}
        return JSON.stringify(null);
    "#, t = target_json);

    let row_val: Value = match session.eval(&row_js, 10) {
        Ok(ref s) if s != "null" && !s.is_empty() => {
            serde_json::from_str(s).unwrap_or(Value::Null)
        }
        _ => Value::Null,
    };

    if row_val.is_null() {
        return (CategoryStats::empty(), String::new());
    }

    let stats = CategoryStats {
        sales_to_one:  row_val["salesToOne"].as_str().unwrap_or("").to_string(),
        sales_to_ten:  row_val["salesToTen"].as_str().unwrap_or("").to_string(),
        publisher_pct: row_val["publisherPct"].as_str().unwrap_or("").to_string(),
        ku_pct:        row_val["kuPct"].as_str().unwrap_or("").to_string(),
    };

    // Click Keywords button and scrape modal
    let keywords = if let (Some(x), Some(y)) = (
        row_val["kCoords"]["x"].as_f64(),
        row_val["kCoords"]["y"].as_f64(),
    ) {
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
    } else {
        String::new()
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
        return JSON.stringify({x:Math.round(r.x+r.width/2), y:Math.round(r.y+r.height/2)});
    "#;
    if let Ok(s) = session.eval(js, 8) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                let _ = session.click(x, y);
            }
        }
    }
}
