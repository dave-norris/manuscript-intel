// competition_analyzer.rs — Competition analysis via PR CSV exports only
//
// Per keyword:
//   1. Competition Analyzer → search → Export Competition Data → Export All → CSV
//   2. Competition Analyzer → Unleash the Categories → Export Categories → CSV
//
// Both CSVs moved to _analysis/competition-csvs/
// Then: parse → fetch covers → AI analysis → competition-report.md + competition-data.json

use std::path::{Path, PathBuf};
use std::fs;
use std::time::Duration;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::cdp;
use crate::commands::call_llm;

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CompetitorBook {
    pub title:        String,
    pub subtitle:     String,
    pub review_score: String,
    pub ratings:      String,
    pub author:       String,
    pub age:          String,
    pub absr:         String,
    pub pages:        String,
    pub kwt:          String,
    pub price:        String,
    pub dy_sales:     String,
    pub mo_sales:     String,
    pub amazon_url:   String,
    pub keyword:      String,
    pub cover_url:    String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CategoryRow {
    pub category:      String,
    pub sales_to_one:  String,
    pub sales_to_ten:  String,
    pub publisher_pct: String,
    pub ku_pct:        String,
    pub keyword:       String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CompetitionData {
    pub generated:         String,
    pub keywords_analyzed: Vec<String>,
    pub books:             Vec<CompetitorBook>,
    pub categories:        Vec<CategoryRow>,
}

#[derive(Serialize)]
pub struct CompetitionResult {
    pub success: bool,
    pub report:  String,
    pub error:   String,
}

#[derive(Deserialize)]
pub struct CompetitionRequest {
    pub folder:   String,
    pub api_key:  String,
    pub model:    String,
    pub store:    String,
    pub provider: String,
}

// ── Command ───────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn analyze_competition(
    app: AppHandle,
    request: CompetitionRequest,
) -> CompetitionResult {
    tokio::task::spawn_blocking(move || {
        run_competition_analysis(&app, &request)
    }).await.unwrap()
}

// ── Core ──────────────────────────────────────────────────────────────────────

fn emit(app: &AppHandle, msg: &str) { let _ = app.emit("genre:log", msg); }

fn run_competition_analysis(app: &AppHandle, req: &CompetitionRequest) -> CompetitionResult {
    let folder       = PathBuf::from(&req.folder);
    let analysis_dir = folder.join("_analysis");

    if !analysis_dir.exists() {
        return err("No _analysis folder. Run Full Analysis and PR Keywords first.");
    }

    let keywords = load_pr_keywords(&analysis_dir);
    if keywords.is_empty() {
        return err("No PR search terms found. Run 'PR Keywords' first.");
    }
    emit(app, &format!("Loaded {} PR keyword(s).", keywords.len()));
    emit(app, &format!("Store: {}", req.store));

    let csvs_dir = analysis_dir.join("competition-csvs");
    if let Err(e) = fs::create_dir_all(&csvs_dir) {
        return err(&format!("Cannot create competition-csvs folder: {}", e));
    }

    // Download directly to the _analysis/competition-csvs/ folder
    // so PR saves files there without any intermediate move or save dialog.
    let downloads = csvs_dir.clone();

    emit(app, "Connecting to Publisher Rocket...");
    let target = match cdp::ensure_rocket() {
        Ok(t) => t,
        Err(e) => return err(&format!("CDP error: {}", e)),
    };
    let mut session = match cdp::connect(&target) {
        Ok(s) => s,
        Err(e) => return err(&format!("CDP connect error: {}", e)),
    };
    emit(app, "  CDP session established.");

    // Set download path before any export — suppresses the save dialog
    let downloads_str = downloads.to_string_lossy().to_string();
    session.set_download_path(&downloads_str);
    emit(app, &format!("  Downloads path set: {}", downloads_str));

    crate::reset_cancel();

    // ── Per-keyword: export both CSVs ─────────────────────────────────────────
    let mut book_csvs:     Vec<(PathBuf, String)> = Vec::new();
    let mut category_csvs: Vec<(PathBuf, String)> = Vec::new();

    for (i, keyword) in keywords.iter().enumerate() {
        if crate::is_cancelled() { emit(app, "⚠ Cancelled."); break; }

        emit(app, &format!("[{}/{}] \"{}\"", i + 1, keywords.len(), keyword));

        // ── Books CSV ─────────────────────────────────────────────────────────
        match export_books_csv(&mut session, keyword, &req.store, &downloads, &csvs_dir, app) {
            Ok(p)  => {
                emit(app, &format!("  ✓ Books CSV: {}", p.file_name().unwrap_or_default().to_string_lossy()));
                book_csvs.push((p, keyword.clone()));
            }
            Err(e) => emit(app, &format!("  ⚠ Books CSV failed: {}", e)),
        }

        // ── Categories CSV (Unleash the Categories) ───────────────────────────
        match export_categories_csv(&mut session, &downloads, &csvs_dir, keyword, app) {
            Ok(p)  => {
                emit(app, &format!("  ✓ Categories CSV: {}", p.file_name().unwrap_or_default().to_string_lossy()));
                category_csvs.push((p, keyword.clone()));
            }
            Err(e) => emit(app, &format!("  ⚠ Categories CSV failed: {}", e)),
        }

        std::thread::sleep(Duration::from_secs(2));
    }

    if book_csvs.is_empty() {
        return err("No book CSVs exported. Check Publisher Rocket is open and logged in.");
    }

    // ── Parse ─────────────────────────────────────────────────────────────────
    emit(app, "Parsing CSVs...");

    let mut all_books: Vec<CompetitorBook> = Vec::new();
    let mut seen_urls: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (path, keyword) in &book_csvs {
        match parse_books_csv(path, keyword) {
            Ok(books) => {
                emit(app, &format!("  {} books — \"{}\"", books.len(), keyword));
                for b in books {
                    if !seen_urls.contains(&b.amazon_url) {
                        seen_urls.insert(b.amazon_url.clone());
                        all_books.push(b);
                    }
                }
            }
            Err(e) => emit(app, &format!("  ⚠ {}", e)),
        }
    }

    let mut all_categories: Vec<CategoryRow> = Vec::new();
    let mut seen_cats: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (path, keyword) in &category_csvs {
        match parse_categories_csv(path, keyword) {
            Ok(cats) => {
                emit(app, &format!("  {} categories — \"{}\"", cats.len(), keyword));
                for c in cats {
                    if !seen_cats.contains(&c.category) {
                        seen_cats.insert(c.category.clone());
                        all_categories.push(c);
                    }
                }
            }
            Err(e) => emit(app, &format!("  ⚠ {}", e)),
        }
    }

    emit(app, &format!("Total: {} unique books, {} unique categories", all_books.len(), all_categories.len()));

    // ── Cover images ──────────────────────────────────────────────────────────
    emit(app, "Fetching cover images...");
    let mut covers_b64: Vec<(String, String)> = Vec::new();

    for book in all_books.iter().take(10) {
        if let Some(asin) = extract_asin(&book.amazon_url) {
            let url = format!("https://images-na.ssl-images-amazon.com/images/P/{}.01.LZZZZZZZ.jpg", asin);
            match fetch_image_b64(&url) {
                Ok(b64) => {
                    emit(app, &format!("  ✓ {}", book.title));
                    covers_b64.push((book.title.clone(), b64));
                }
                Err(e) => emit(app, &format!("  ⚠ {}: {}", book.title, e)),
            }
        }
    }

    // ── Save raw data ─────────────────────────────────────────────────────────
    let data = CompetitionData {
        generated:         chrono::Utc::now().to_rfc3339(),
        keywords_analyzed: keywords.clone(),
        books:             all_books.clone(),
        categories:        all_categories.clone(),
    };
    if let Ok(json) = serde_json::to_string_pretty(&data) {
        let _ = fs::write(analysis_dir.join("competition-data.json"), &json);
        emit(app, "  ✓ competition-data.json saved.");
    }

    // ── AI analysis ───────────────────────────────────────────────────────────
    emit(app, "Running AI analysis...");
    let analysis_model = &req.model;
    emit(app, &format!("  Using {}", analysis_model));

    let genre_context = fs::read_to_string(analysis_dir.join("genre-analysis.md"))
        .unwrap_or_default();

    match call_competition_ai(
        &req.provider, &req.api_key, analysis_model,
        &all_books, &all_categories,
        &covers_b64, &genre_context, &keywords,
    ) {
        Err(e) => err(&format!("AI error: {}", e)),
        Ok(report) => {
            let _ = fs::write(analysis_dir.join("competition-report.md"), &report);
            emit(app, "✓ competition-report.md saved.");
            CompetitionResult { success: true, report, error: String::new() }
        }
    }
}

// ── CDP: export books CSV ─────────────────────────────────────────────────────

fn export_books_csv(
    session: &mut cdp::Session,
    keyword: &str,
    store: &str,
    downloads: &Path,
    _csvs_dir: &Path,
    app: &AppHandle,
) -> Result<PathBuf, String> {
    nav_competition(session)?;
    open_new_search(session)?;
    type_keyword(session, keyword)?;
    std::thread::sleep(Duration::from_millis(300));
    set_store_dropdown(session, store);
    std::thread::sleep(Duration::from_millis(300));
    click_go(session)?;
    emit(app, "  Waiting for results...");
    wait_for_results(session)?;

    // Export Competition Data → Export All
    click_button_by_exact_text(session, "Export Competition Data")?;
    std::thread::sleep(Duration::from_millis(800));
    click_button_by_exact_text(session, "Export All")?;
    emit(app, "  Waiting for books CSV...");

    let csv = wait_for_new_csv(downloads, 15)?;
    Ok(csv)
}

// ── CDP: export categories CSV (Unleash the Categories) ───────────────────────

fn export_categories_csv(
    session: &mut cdp::Session,
    downloads: &Path,
    _csvs_dir: &Path,
    _keyword: &str,
    app: &AppHandle,
) -> Result<PathBuf, String> {
    // We should already be on Competition Analyzer results — click Unleash the Categories
    click_button_by_exact_text(session, "Unleash the Categories")
        .map_err(|_| "Unleash the Categories button not found — run books export first".to_string())?;

    // Wait for categories page (Export Categories button appears)
    emit(app, "  Waiting for categories page...");
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(500));
        let v = session.eval(r#"
            Array.from(document.querySelectorAll('button'))
              .find(b => b.textContent.trim() === 'Export Categories') ? 'ready' : 'waiting'
        "#, 5).unwrap_or_default();
        if v.contains("ready") { break; }
    }

    // Click Export Categories
    click_button_by_exact_text(session, "Export Categories")?;
    emit(app, "  Waiting for categories CSV...");

    let csv = wait_for_new_csv(downloads, 15)?;

    // Go back to results for next keyword
    click_button_by_exact_text(session, "Back").ok();
    std::thread::sleep(Duration::from_millis(500));

    Ok(csv)
}

// ── CDP helpers ───────────────────────────────────────────────────────────────

fn nav_competition(session: &mut cdp::Session) -> Result<(), String> {
    let _ = session.eval("window.location.hash = '#/keyword';", 5);
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(400));
        let v = session.eval(r#"
            Array.from(document.querySelectorAll('.menubar-selection'))
              .find(e => e.textContent.trim() === 'Competition Analyzer') ? 'ready' : 'waiting'
        "#, 5).unwrap_or_default();
        if v.contains("ready") { break; }
    }
    let js = r#"
        const el = Array.from(document.querySelectorAll('.menubar-selection'))
          .find(e => e.textContent.trim() === 'Competition Analyzer');
        if (!el) return JSON.stringify(null);
        const r = el.getBoundingClientRect();
        return JSON.stringify({x:Math.round(r.x+r.width/2),y:Math.round(r.y+r.height/2)});
    "#;
    if let Ok(s) = session.eval(js, 8) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                let _ = session.click(x, y);
                for _ in 0..10 {
                    std::thread::sleep(Duration::from_millis(400));
                    let h = session.eval("window.location.hash", 5).unwrap_or_default();
                    if h.contains("competition") { return Ok(()); }
                }
                return Ok(());
            }
        }
    }
    Err("Competition Analyzer nav not found".to_string())
}

fn open_new_search(session: &mut cdp::Session) -> Result<(), String> {
    let js = r#"
        const btn = Array.from(document.querySelectorAll('button'))
          .find(b => b.textContent.trim().includes('New') &&
                    (b.textContent.includes('Analysis') || b.textContent.includes('Search') ||
                     b.textContent.includes('Competitor')));
        if (!btn) return JSON.stringify(null);
        const r = btn.getBoundingClientRect();
        return JSON.stringify({x:Math.round(r.x+r.width/2),y:Math.round(r.y+r.height/2)});
    "#;
    if let Ok(s) = session.eval(js, 8) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                let _ = session.click(x, y);
                std::thread::sleep(Duration::from_millis(800));
                return Ok(());
            }
        }
    }
    // Already on fresh state
    let has = session.eval(r#"document.querySelector('input[placeholder*="keyword"],input[placeholder*="Keyword"]') ? 'yes' : 'no'"#, 5).unwrap_or_default();
    if has.contains("yes") { return Ok(()); }
    Err("New search button not found".to_string())
}

fn type_keyword(session: &mut cdp::Session, keyword: &str) -> Result<(), String> {
    for _ in 0..8 {
        let v = session.eval(r#"document.querySelector('input[placeholder*="keyword"],input[placeholder*="Keyword"],input[type="text"]') ? 'found' : 'waiting'"#, 5).unwrap_or_default();
        if v.contains("found") { break; }
        std::thread::sleep(Duration::from_millis(400));
    }
    let js = r#"
        const input = document.querySelector('input[placeholder*="keyword"],input[placeholder*="Keyword"],input[type="text"]');
        if (!input) return JSON.stringify(null);
        input.focus();
        const r = input.getBoundingClientRect();
        return JSON.stringify({x:Math.round(r.x+r.width/2),y:Math.round(r.y+r.height/2)});
    "#;
    if let Ok(s) = session.eval(js, 8) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                let _ = session.click(x, y);
                std::thread::sleep(Duration::from_millis(200));
                session.select_all();
                std::thread::sleep(Duration::from_millis(100));
                session.send_backspace();
                std::thread::sleep(Duration::from_millis(100));
                for ch in keyword.chars() {
                    session.send_char(ch);
                    std::thread::sleep(Duration::from_millis(40));
                }
                return Ok(());
            }
        }
    }
    Err("Keyword input not found".to_string())
}

fn set_store_dropdown(session: &mut cdp::Session, store: &str) {
    let sj = serde_json::to_string(store).unwrap();
    // Find a <select> that contains an option matching the store label,
    // then select that option by matching text content.
    let js = format!(r#"
        const wanted = {s}.toLowerCase();
        const selects = Array.from(document.querySelectorAll('select'));
        for (const sel of selects) {{
            const opts = Array.from(sel.options);
            const match = opts.find(o => o.textContent.trim().toLowerCase().includes(wanted));
            if (match) {{
                const setter = Object.getOwnPropertyDescriptor(window.HTMLSelectElement.prototype,'value').set;
                setter.call(sel, match.value);
                sel.dispatchEvent(new Event('change',{{bubbles:true}}));
                return sel.value;
            }}
        }}
        // Fallback: try the first select and set value directly
        const sel = selects[0];
        if (!sel) return 'no select';
        const setter = Object.getOwnPropertyDescriptor(window.HTMLSelectElement.prototype,'value').set;
        setter.call(sel, {s});
        sel.dispatchEvent(new Event('change',{{bubbles:true}}));
        return sel.value;
    "#, s = sj);
    let _ = session.eval(&js, 5);
}

fn click_go(session: &mut cdp::Session) -> Result<(), String> {
    let js = r#"
        const btn = Array.from(document.querySelectorAll('button'))
          .find(b => b.textContent.trim().includes('Go') || b.textContent.trim() === 'Search');
        if (!btn) return JSON.stringify(null);
        const r = btn.getBoundingClientRect();
        return JSON.stringify({x:Math.round(r.x+r.width/2),y:Math.round(r.y+r.height/2)});
    "#;
    if let Ok(s) = session.eval(js, 8) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                let _ = session.click(x, y);
                return Ok(());
            }
        }
    }
    Err("Go button not found".to_string())
}

fn wait_for_results(session: &mut cdp::Session) -> Result<(), String> {
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(500));
        let v = session.eval(r#"Array.from(document.querySelectorAll('button')).find(b=>b.textContent.trim()==='Export Competition Data')?'ready':'waiting'"#, 5).unwrap_or_default();
        if v.contains("ready") { return Ok(()); }
    }
    Err("Results did not load in 20s".to_string())
}

/// Click any button whose exact trimmed text matches.
fn click_button_by_exact_text(session: &mut cdp::Session, text: &str) -> Result<(), String> {
    let tj = serde_json::to_string(text).unwrap();
    let js = format!(r#"
        const btn = Array.from(document.querySelectorAll('button'))
          .find(b => b.textContent.trim() === {t});
        if (!btn) return JSON.stringify(null);
        btn.scrollIntoView({{block:'center'}});
        const r = btn.getBoundingClientRect();
        return JSON.stringify({{x:Math.round(r.x+r.width/2),y:Math.round(r.y+r.height/2)}});
    "#, t = tj);
    // Poll up to 5s for the button to appear
    for _ in 0..10 {
        if let Ok(s) = session.eval(&js, 5) {
            if let Ok(v) = serde_json::from_str::<Value>(&s) {
                if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                    let _ = session.click(x, y);
                    std::thread::sleep(Duration::from_millis(300));
                    return Ok(());
                }
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(format!("Button '{}' not found", text))
}

fn wait_for_new_csv(downloads: &Path, timeout_secs: u64) -> Result<PathBuf, String> {
    // Snapshot by (path, mtime) — catches overwrites of existing files
    let before = snapshot_csvs(downloads);
    let start = std::time::Instant::now();
    while start.elapsed().as_secs() < timeout_secs {
        std::thread::sleep(Duration::from_millis(500));
        let after = snapshot_csvs(downloads);
        // A file is "new" if its (path, mtime) wasn't in the before snapshot
        let mut new: Vec<PathBuf> = after.iter()
            .filter(|(path, mtime)| !before.contains(&(path.clone(), *mtime)))
            .map(|(path, _)| path.clone())
            .collect();
        if !new.is_empty() {
            // Return the most recently modified
            new.sort_by_key(|p| std::cmp::Reverse(
                p.metadata().and_then(|m| m.modified())
                 .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
            ));
            return Ok(new.remove(0));
        }
    }
    Err(format!("No CSV appeared in {} within {}s",
        downloads.display(), timeout_secs))
}

fn snapshot_csvs(dir: &Path) -> std::collections::HashSet<(PathBuf, u64)> {
    fs::read_dir(dir).map(|e| e.flatten()
        .filter(|e| e.file_name().to_string_lossy().to_lowercase().ends_with(".csv"))
        .map(|e| {
            let path = e.path();
            let mtime = path.metadata()
                .and_then(|m| m.modified())
                .map(|t| t.duration_since(std::time::SystemTime::UNIX_EPOCH)
                          .map(|d| d.as_secs()).unwrap_or(0))
                .unwrap_or(0);
            (path, mtime)
        })
        .collect()
    ).unwrap_or_default()
}

// ── CSV Parsers ───────────────────────────────────────────────────────────────

fn parse_books_csv(path: &Path, keyword: &str) -> Result<Vec<CompetitorBook>, String> {
    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let content = content.trim_start_matches('\u{feff}');
    let mut books = Vec::new();
    let mut lines = content.lines();
    lines.next(); // skip header
    for line in lines {
        if line.trim().is_empty() { continue; }
        let f = parse_csv_line(line);
        if f.len() < 13 { continue; }
        books.push(CompetitorBook {
            title: f[0].clone(), subtitle: f[1].clone(),
            review_score: f[2].clone(), ratings: f[3].clone(),
            author: f[4].clone(), age: f[5].clone(),
            absr: f[6].clone(), pages: f[7].clone(),
            kwt: f[8].clone(), price: f[9].clone(),
            dy_sales: f[10].clone(), mo_sales: f[11].clone(),
            amazon_url: f[12].clone(),
            keyword: keyword.to_string(), cover_url: String::new(),
        });
    }
    Ok(books)
}

fn parse_categories_csv(path: &Path, keyword: &str) -> Result<Vec<CategoryRow>, String> {
    // Columns: category, sales1, sales10, largePublisher, kindleUnlimited
    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let content = content.trim_start_matches('\u{feff}');
    let mut rows = Vec::new();
    let mut lines = content.lines();
    lines.next(); // skip header
    for line in lines {
        if line.trim().is_empty() { continue; }
        let f = parse_csv_line(line);
        if f.len() < 5 { continue; }
        rows.push(CategoryRow {
            category:      f[0].clone(),
            sales_to_one:  f[1].clone(),
            sales_to_ten:  f[2].clone(),
            publisher_pct: f[3].clone(),
            ku_pct:        f[4].clone(),
            keyword:       keyword.to_string(),
        });
    }
    Ok(rows)
}

fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                if in_quotes && chars.peek() == Some(&'"') { chars.next(); current.push('"'); }
                else { in_quotes = !in_quotes; }
            }
            ',' if !in_quotes => { fields.push(current.trim().to_string()); current = String::new(); }
            _ => current.push(ch),
        }
    }
    fields.push(current.trim().to_string());
    fields
}

// ── AI Analysis ───────────────────────────────────────────────────────────────

fn call_competition_ai(
    provider: &str,
    api_key: &str,
    model: &str,
    books: &[CompetitorBook],
    categories: &[CategoryRow],
    covers: &[(String, String)],
    genre_context: &str,
    keywords: &[String],
) -> Result<String, String> {

    // Build book summary
    let mut book_text = String::new();
    for (i, b) in books.iter().take(20).enumerate() {
        book_text.push_str(&format!(
            "{}. \"{}\"{}  by {}  ABSR:{}  {} ratings ({})  {}  MO:{}\n",
            i + 1, b.title,
            if b.subtitle.is_empty() { String::new() } else { format!(" ({})", b.subtitle) },
            b.author, b.absr, b.ratings, b.review_score, b.price, b.mo_sales
        ));
    }

    // Build category summary — sort by sales_to_ten ascending (easiest to rank)
    let mut cat_rows = categories.to_vec();
    cat_rows.sort_by(|a, b| {
        let av: u64 = a.sales_to_ten.parse().unwrap_or(999999);
        let bv: u64 = b.sales_to_ten.parse().unwrap_or(999999);
        av.cmp(&bv)
    });

    let mut cat_text = String::new();
    for c in cat_rows.iter().take(20) {
        cat_text.push_str(&format!(
            "  {} | #1:{} #10:{} | Pub:{}  KU:{}\n",
            c.category, c.sales_to_one, c.sales_to_ten, c.publisher_pct, c.ku_pct
        ));
    }

    let system = r#"You are a book publishing strategist helping an indie author maximize sales.

You have two data sources from Publisher Rocket:
1. Competition Analyzer: the top competing books for relevant keywords
2. Unleash Categories: all categories those books rank in, with competition stats

Use both to produce a BRIEF, ACTIONABLE positioning brief.

Return ONLY a JSON object — no markdown, no preamble:
{
  "best_categories": ["Full > Category > Path — reason it's good (sales#1/sales#10)", "(2-3 best opportunities)"],
  "avoid_categories": ["Category — reason (too competitive or wrong fit)"],
  "cover": "2-3 bullet points on cover design patterns",
  "title": "2-3 bullet points on title conventions",
  "subtitle": "2-3 bullet points on subtitle formula with example",
  "series": "1-2 bullet points on series naming",
  "pricing": "ebook and print sweet spots with specific $ numbers",
  "positioning": "2-3 bullet points on differentiation",
  "top_comps": ["Title by Author — why it matters", "(up to 3)"],
  "summary": "One paragraph: what matters most for this book's success"
}"#;

    let user_text = format!(
        "Genre: {}\nKeywords analyzed: {}\n\nCOMPETING BOOKS:\n{}\nCATEGORY OPPORTUNITIES (sorted easiest to rank first):\n{}",
        if genre_context.len() > 600 { &genre_context[..600] } else { genre_context },
        keywords.join(", "),
        book_text,
        cat_text
    );

    let raw = if covers.is_empty() {
        call_llm(provider, api_key, model, system, &user_text, 1200)?
    } else {
        // Multimodal request with cover images
        match provider {
            "tokenmix" => {
                // OpenAI-compatible format for images
                let mut parts: Vec<Value> = vec![json!({"type":"text","text":user_text})];
                for (title, b64) in covers.iter().take(8) {
                    parts.push(json!({"type":"text","text":format!("Cover: {}", title)}));
                    parts.push(json!({"type":"image_url","image_url":{"url":format!("data:image/jpeg;base64,{}", b64)}}));
                }
                let body = json!({
                    "model": model, "max_tokens": 1200,
                    "messages": [
                        {"role":"system","content":system},
                        {"role":"user","content":parts}
                    ]
                });
                let resp = reqwest::blocking::Client::builder()
                    .timeout(Duration::from_secs(120)).build().unwrap()
                    .post("https://api.tokenmix.ai/v1/chat/completions")
                    .header("Authorization", format!("Bearer {}", api_key))
                    .header("content-type", "application/json")
                    .json(&body).send().map_err(|e| format!("API: {}", e))?;
                let jv: Value = resp.json().map_err(|e| format!("Parse: {}", e))?;
                if let Some(e) = jv.get("error") {
                    return Err(format!("TokenMix: {}", e["message"].as_str().unwrap_or("unknown")));
                }
                jv["choices"][0]["message"]["content"].as_str().ok_or("Empty response")?.to_string()
            }
            _ => {
                // Anthropic format for images
                let mut parts: Vec<Value> = vec![json!({"type":"text","text":user_text})];
                for (title, b64) in covers.iter().take(8) {
                    parts.push(json!({"type":"text","text":format!("Cover: {}", title)}));
                    parts.push(json!({"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":b64}}));
                }
                let body = json!({
                    "model": model, "max_tokens": 1200,
                    "system": system,
                    "messages": [{"role":"user","content":parts}]
                });
                let resp = reqwest::blocking::Client::builder()
                    .timeout(Duration::from_secs(120)).build().unwrap()
                    .post("https://api.anthropic.com/v1/messages")
                    .header("x-api-key", api_key)
                    .header("anthropic-version", "2023-06-01")
                    .header("content-type", "application/json")
                    .json(&body).send().map_err(|e| format!("API: {}", e))?;
                let jv: Value = resp.json().map_err(|e| format!("Parse: {}", e))?;
                if let Some(e) = jv.get("error") {
                    return Err(format!("Claude: {}", e["message"].as_str().unwrap_or("unknown")));
                }
                jv["content"][0]["text"].as_str().ok_or("Empty response")?.to_string()
            }
        }
    };

    build_report(&raw, books, categories, keywords)
}

fn build_report(raw: &str, books: &[CompetitorBook], categories: &[CategoryRow], keywords: &[String]) -> Result<String, String> {
    let clean = raw.trim()
        .trim_start_matches("```json").trim_start_matches("```")
        .trim_end_matches("```").trim();

    let v: Value = serde_json::from_str(clean)
        .map_err(|e| format!("JSON parse: {} | got: {}", e, &clean[..clean.len().min(300)]))?;

    let str_field  = |key: &str| v[key].as_str().unwrap_or("").to_string();
    let arr_field  = |key: &str| -> Vec<String> {
        v[key].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).map(String::from).collect()).unwrap_or_default()
    };

    let now = chrono::Utc::now().format("%B %-d, %Y %H:%M UTC").to_string();

    let mut md = vec![
        "# Competition Analysis".to_string(),
        format!("Generated: {}", now),
        format!("Keywords: {}", keywords.join(", ")),
        format!("Books analyzed: {}  |  Categories analyzed: {}", books.len().min(20), categories.len()),
        String::new(),
        "---".to_string(),
        String::new(),
    ];

    // Summary
    let summary = str_field("summary");
    if !summary.is_empty() {
        md.push(format!("> {}", summary));
        md.push(String::new());
        md.push("---".to_string());
        md.push(String::new());
    }

    // Best categories — the most actionable section
    let best_cats = arr_field("best_categories");
    if !best_cats.is_empty() {
        md.push("## Best Categories to Target".to_string());
        md.push(String::new());
        for c in &best_cats { md.push(format!("- {}", c)); }
        md.push(String::new());
    }

    let avoid_cats = arr_field("avoid_categories");
    if !avoid_cats.is_empty() {
        md.push("## Categories to Avoid".to_string());
        md.push(String::new());
        for c in &avoid_cats { md.push(format!("- {}", c)); }
        md.push(String::new());
    }

    // Top comps
    let comps = arr_field("top_comps");
    if !comps.is_empty() {
        md.push("## Study These Books".to_string());
        md.push(String::new());
        for c in &comps { md.push(format!("- {}", c)); }
        md.push(String::new());
    }

    // Remaining sections
    for (key, label) in &[
        ("cover",       "## Cover Design"),
        ("title",       "## Title"),
        ("subtitle",    "## Subtitle"),
        ("series",      "## Series"),
        ("pricing",     "## Pricing"),
        ("positioning", "## Positioning"),
    ] {
        let text = str_field(key);
        if text.is_empty() { continue; }
        md.push(label.to_string());
        md.push(String::new());
        for line in text.lines() {
            let t = line.trim();
            if t.is_empty() { continue; }
            if t.starts_with('-') || t.starts_with('•') { md.push(t.to_string()); }
            else { md.push(format!("- {}", t)); }
        }
        md.push(String::new());
    }

    Ok(md.join("\n"))
}

// ── Helpers ───────────────────────────────────────────────────────────────────


fn load_pr_keywords(analysis_dir: &Path) -> Vec<String> {
    if let Ok(text) = fs::read_to_string(analysis_dir.join("pr-keywords.json")) {
        if let Ok(kws) = serde_json::from_str::<Vec<String>>(&text) {
            if !kws.is_empty() { return kws; }
        }
    }
    Vec::new()
}

fn extract_asin(url: &str) -> Option<String> {
    let re = regex::Regex::new(r"/(?:dp|product)/([A-Z0-9]{10})").ok()?;
    re.captures(url)?.get(1).map(|m| m.as_str().to_string())
}

fn fetch_image_b64(url: &str) -> Result<String, String> {
    let bytes = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10)).user_agent("Mozilla/5.0").build().unwrap()
        .get(url).send().map_err(|e| e.to_string())?
        .bytes().map_err(|e| e.to_string())?;
    use base64::Engine;
    Ok(base64::engine::general_purpose::STANDARD.encode(&bytes))
}

fn err(msg: &str) -> CompetitionResult {
    CompetitionResult { success: false, report: String::new(), error: msg.to_string() }
}
