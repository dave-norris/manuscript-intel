// competition_analyzer.rs — Competitive positioning analysis
//
// Flow:
//   1. Read _analysis/kdp-keywords.md for the 7 optimized keyword strings
//   2. For each keyword: open PR Competition Analyzer, type keyword, scrape 10 books
//   3. For each book ASIN: fetch Amazon product page → subtitle, description, series
//   4. Fetch cover images (base64) for top 5 books per keyword
//   5. Send all text data + covers to Claude → unified brief report
//   6. Save full data to _analysis/competition-data.json
//   7. Save brief report to _analysis/competition-report.md

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::fs;
use std::time::Duration;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::cdp;
use crate::commands::call_anthropic;
use crate::models;

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CompetitorBook {
    pub title:        String,
    pub subtitle:     String,
    pub author:       String,
    pub asin:         String,
    pub rank:         String,
    pub reviews:      String,
    pub rating:       String,
    pub price:        String,
    pub monthly_sales: String,
    pub cover_url:    String,
    pub description:  String,
    pub series:       String,
    pub pub_date:     String,
    pub page_count:   String,
    pub keyword:      String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CompetitionData {
    pub generated: String,
    pub keywords_analyzed: Vec<String>,
    pub books: Vec<CompetitorBook>,
}

#[derive(Serialize)]
pub struct CompetitionResult {
    pub success: bool,
    pub report:  String,
    pub error:   String,
}

#[derive(Deserialize)]
pub struct CompetitionRequest {
    pub folder:  String,
    pub api_key: String,
    pub model:   String,
    pub store:   String, // "Books" or "Kindle"
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
        return err("No _analysis folder. Run Full Analysis first.");
    }

    crate::reset_cancel();

    // ── Step 1: Load keywords ─────────────────────────────────────────────────
    let keywords = load_keywords(&analysis_dir);
    if keywords.is_empty() {
        return err("No PR search terms found. Run \'Generate PR Keywords\' first.");
    }
    emit(app, &format!("Loaded {} keyword strings to analyze.", keywords.len()));

    // ── Step 2: Connect to Publisher Rocket ───────────────────────────────────
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

    // ── Step 3: Competition Analyzer per keyword ──────────────────────────────
    let mut all_books: Vec<CompetitorBook> = Vec::new();
    let mut analyzed_keywords: Vec<String> = Vec::new();
    let mut seen_asins: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (i, keyword) in keywords.iter().enumerate() {
        emit(app, &format!("[{}/{}] Analyzing: \"{}\"", i + 1, keywords.len(), keyword));

        let books = scrape_pr_competition(&mut session, keyword, &req.store, app);
        emit(app, &format!("  {} books found.", books.len()));

        for book in books {
            // Deduplicate by ASIN across keywords
            if !seen_asins.contains(&book.asin) && !book.asin.is_empty() {
                seen_asins.insert(book.asin.clone());
                all_books.push(book);
            }
        }
        analyzed_keywords.push(keyword.clone());

        // Small delay between keywords
        std::thread::sleep(Duration::from_secs(2));

        // Check for cancellation
        if crate::is_cancelled() {
            emit(app, "⚠ Cancelled by user.");
            break;
        }
    }

    emit(app, &format!("Total unique books scraped: {}", all_books.len()));

    // ── Step 4: Enrich with Amazon product pages ──────────────────────────────
    emit(app, "Fetching Amazon product pages for top books...");
    let to_enrich: Vec<_> = all_books.iter()
        .filter(|b| !b.asin.is_empty())
        .take(20) // cap at 20 to keep it fast
        .map(|b| b.asin.clone())
        .collect();

    let amazon_data = fetch_amazon_pages(&to_enrich, app);

    // Merge Amazon data back into books
    for book in &mut all_books {
        if let Some(data) = amazon_data.get(&book.asin) {
            if book.subtitle.is_empty() {
                book.subtitle = data.get("subtitle").cloned().unwrap_or_default();
            }
            if book.description.is_empty() {
                book.description = data.get("description").cloned().unwrap_or_default();
            }
            if book.series.is_empty() {
                book.series = data.get("series").cloned().unwrap_or_default();
            }
            if book.pub_date.is_empty() {
                book.pub_date = data.get("pub_date").cloned().unwrap_or_default();
            }
            if book.page_count.is_empty() {
                book.page_count = data.get("page_count").cloned().unwrap_or_default();
            }
        }
    }

    // ── Step 5: Fetch cover images (top 10 unique books) ─────────────────────
    emit(app, "Fetching cover images...");
    let cover_books: Vec<_> = all_books.iter()
        .filter(|b| !b.cover_url.is_empty())
        .take(10)
        .collect();

    let mut covers_b64: Vec<(String, String)> = Vec::new(); // (title, base64)
    for book in &cover_books {
        match fetch_image_b64(&book.cover_url) {
            Ok(b64) => {
                emit(app, &format!("  ✓ Cover: {}", book.title));
                covers_b64.push((book.title.clone(), b64));
            }
            Err(e) => emit(app, &format!("  ⚠ Cover fetch failed for {}: {}", book.title, e)),
        }
    }
    emit(app, &format!("  {} covers fetched.", covers_b64.len()));

    // ── Step 6: Save raw data ─────────────────────────────────────────────────
    let competition_data = CompetitionData {
        generated: chrono::Utc::now().to_rfc3339(),
        keywords_analyzed: analyzed_keywords.clone(),
        books: all_books.clone(),
    };
    if let Ok(json) = serde_json::to_string_pretty(&competition_data) {
        let _ = fs::write(analysis_dir.join("competition-data.json"), json);
        emit(app, "  ✓ competition-data.json saved.");
    }

    // ── Step 7: AI analysis ───────────────────────────────────────────────────
    emit(app, "Sending data to AI for competitive analysis...");
    let analysis_model = models::resolve_analysis_model(&req.model);
    emit(app, &format!("  Using {}", analysis_model));

    // Load genre context
    let genre_context = fs::read_to_string(analysis_dir.join("genre-analysis.md"))
        .unwrap_or_default();

    match call_competition_ai(
        &req.api_key,
        analysis_model,
        &all_books,
        &covers_b64,
        &genre_context,
        &analyzed_keywords,
    ) {
        Err(e) => err(&format!("AI analysis error: {}", e)),
        Ok(report) => {
            let _ = fs::write(analysis_dir.join("competition-report.md"), &report);
            emit(app, "✓ competition-report.md saved.");
            CompetitionResult { success: true, report, error: String::new() }
        }
    }
}

// ── PR Competition Analyzer scraping ─────────────────────────────────────────

fn scrape_pr_competition(
    session: &mut cdp::Session,
    keyword: &str,
    store: &str,
    app: &AppHandle,
) -> Vec<CompetitorBook> {

    // Navigate to Competition Analyzer
    let nav_js = r#"
        const el = Array.from(document.querySelectorAll('p,span,div,a,li'))
          .find(e => e.children.length === 0 &&
            (e.textContent.trim() === 'Competition Analyzer' ||
             e.textContent.trim() === 'Competitor Analysis'));
        if (!el) return JSON.stringify(null);
        el.scrollIntoView({block:'center'});
        const r = el.getBoundingClientRect();
        return JSON.stringify({x:Math.round(r.x+r.width/2), y:Math.round(r.y+r.height/2)});
    "#;

    if let Ok(s) = session.eval(nav_js, 8) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                let _ = session.click(x, y);
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    }

    // Look for "New Competitor Analysis" button or the dialog input
    let new_btn_js = r#"
        const btn = Array.from(document.querySelectorAll('button,a,div,span'))
          .find(e => e.textContent.trim() === 'New Competitor Analysis' ||
                     e.textContent.trim() === 'New Analysis' ||
                     e.textContent.trim().includes('New Competitor'));
        if (!btn) return JSON.stringify(null);
        btn.scrollIntoView({block:'center'});
        const r = btn.getBoundingClientRect();
        return JSON.stringify({x:Math.round(r.x+r.width/2), y:Math.round(r.y+r.height/2)});
    "#;

    if let Ok(s) = session.eval(new_btn_js, 8) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                let _ = session.click(x, y);
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }

    // Type keyword into dialog input
    let kw_json = serde_json::to_string(keyword).unwrap();
    let type_js = format!(r#"
        const input = document.querySelector('input[placeholder*="keyword"], input[type="text"]');
        if (!input) return 'no input';
        input.focus();
        const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype,'value').set;
        setter.call(input, {kw});
        input.dispatchEvent(new Event('input', {{bubbles:true}}));
        input.dispatchEvent(new Event('change', {{bubbles:true}}));
        return 'ok';
    "#, kw = kw_json);
    let _ = session.eval(&type_js, 8);
    std::thread::sleep(Duration::from_millis(500));

    // Set store dropdown
    let store_json = serde_json::to_string(store).unwrap();
    let store_js = format!(r#"
        const sel = document.querySelector('select');
        if (!sel) return 'no select';
        const setter = Object.getOwnPropertyDescriptor(window.HTMLSelectElement.prototype,'value').set;
        setter.call(sel, {s});
        sel.dispatchEvent(new Event('change', {{bubbles:true}}));
        return 'ok';
    "#, s = store_json);
    let _ = session.eval(&store_js, 8);
    std::thread::sleep(Duration::from_millis(300));

    // Click "Go Get Em Rocket!"
    let go_js = r#"
        const btn = Array.from(document.querySelectorAll('button'))
          .find(b => b.textContent.trim().includes('Go Get Em') ||
                     b.textContent.trim().includes('Go!') ||
                     b.textContent.trim() === 'Search');
        if (!btn) return JSON.stringify(null);
        const r = btn.getBoundingClientRect();
        return JSON.stringify({x:Math.round(r.x+r.width/2), y:Math.round(r.y+r.height/2)});
    "#;

    if let Ok(s) = session.eval(go_js, 8) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) {
                let _ = session.click(x, y);
            }
        }
    }

    // Wait for results
    emit(app, "  Waiting for results...");
    std::thread::sleep(Duration::from_secs(8));

    // Scrape results table
    let scrape_js = format!(r#"
        const kw = {kw};
        const rows = Array.from(document.querySelectorAll('tr')).slice(1);
        const books = [];
        for (const row of rows) {{
            const cells = Array.from(row.querySelectorAll('td'));
            if (cells.length < 3) continue;
            const img   = row.querySelector('img');
            const link  = row.querySelector('a[href*="amazon.com"],a[href*="/dp/"]');
            const asin  = link ? (link.href.match(/\/dp\/([A-Z0-9]{{10}})/) || [])[1] || '' : '';
            const cover = img ? img.src : '';
            books.push({{
                title:        cells[0]?.querySelector('p,span,div,a')?.textContent.trim() || cells[0]?.textContent.trim() || '',
                author:       cells[1]?.textContent.trim() || '',
                rank:         cells[2]?.textContent.trim() || '',
                reviews:      cells[3]?.textContent.trim() || '',
                rating:       cells[4]?.textContent.trim() || '',
                price:        cells[5]?.textContent.trim() || '',
                monthly_sales: cells[6]?.textContent.trim() || '',
                asin,
                cover_url: cover,
                keyword: kw,
            }});
        }}
        return JSON.stringify(books.slice(0, 10));
    "#, kw = kw_json);

    let books: Vec<CompetitorBook> = match session.eval(&scrape_js, 15) {
        Ok(ref s) if !s.is_empty() && s != "null" => {
            match serde_json::from_str::<Vec<Value>>(s) {
                Ok(vals) => vals.iter().map(|v| CompetitorBook {
                    title:         v["title"].as_str().unwrap_or("").to_string(),
                    author:        v["author"].as_str().unwrap_or("").to_string(),
                    rank:          v["rank"].as_str().unwrap_or("").to_string(),
                    reviews:       v["reviews"].as_str().unwrap_or("").to_string(),
                    rating:        v["rating"].as_str().unwrap_or("").to_string(),
                    price:         v["price"].as_str().unwrap_or("").to_string(),
                    monthly_sales: v["monthly_sales"].as_str().unwrap_or("").to_string(),
                    asin:          v["asin"].as_str().unwrap_or("").to_string(),
                    cover_url:     v["cover_url"].as_str().unwrap_or("").to_string(),
                    keyword:       v["keyword"].as_str().unwrap_or("").to_string(),
                    subtitle:      String::new(),
                    description:   String::new(),
                    series:        String::new(),
                    pub_date:      String::new(),
                    page_count:    String::new(),
                }).filter(|b| !b.title.is_empty()).collect(),
                Err(_) => Vec::new(),
            }
        }
        _ => Vec::new(),
    };

    books
}

// ── Amazon page fetching ──────────────────────────────────────────────────────

fn fetch_amazon_pages(asins: &[String], app: &AppHandle) -> HashMap<String, HashMap<String, String>> {
    let mut results = HashMap::new();

    for asin in asins {
        emit(app, &format!("  Fetching Amazon page: {}", asin));
        match fetch_amazon_page(asin) {
            Ok(data) => { results.insert(asin.clone(), data); }
            Err(e)   => emit(app, &format!("  ⚠ {}: {}", asin, e)),
        }
        // Polite delay
        std::thread::sleep(Duration::from_millis(800));
    }

    results
}

fn fetch_amazon_page(asin: &str) -> Result<HashMap<String, String>, String> {
    let url = format!("https://www.amazon.com/dp/{}", asin);

    let body = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36")
        .build()
        .unwrap()
        .get(&url)
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .map_err(|e| e.to_string())?
        .text()
        .map_err(|e| e.to_string())?;

    let mut data = HashMap::new();

    // Extract subtitle — appears after title in <span id="productSubtitle">
    if let Some(sub) = extract_between(&body, r#"id="productSubtitle""#, "</span>") {
        let clean = strip_html(&sub).trim().to_string();
        if !clean.is_empty() { data.insert("subtitle".to_string(), clean); }
    }

    // Extract series
    if let Some(series) = extract_between(&body, r#"id="seriesBulletWidget""#, "</span>") {
        let clean = strip_html(&series).trim().to_string();
        if !clean.is_empty() { data.insert("series".to_string(), clean); }
    }

    // Extract description — in <div id="bookDescription_feature_div">
    if let Some(desc) = extract_between(&body, r#"id="bookDescription_feature_div""#, "</div>") {
        let clean = strip_html(&desc).trim().to_string();
        // Truncate to ~500 chars for AI
        let truncated = if clean.len() > 500 { clean[..500].to_string() + "…" } else { clean };
        if !truncated.is_empty() { data.insert("description".to_string(), truncated); }
    }

    // Extract pub date
    if let Some(pub_date) = extract_between(&body, "Publication date", "</span>") {
        let clean = strip_html(&pub_date).trim()
            .trim_start_matches(':').trim().to_string();
        if !clean.is_empty() && clean.len() < 30 {
            data.insert("pub_date".to_string(), clean);
        }
    }

    // Extract page count
    if let Some(pages) = extract_between(&body, "Print length", "</span>") {
        let clean = strip_html(&pages).trim()
            .trim_start_matches(':').trim().to_string();
        if !clean.is_empty() && clean.len() < 20 {
            data.insert("page_count".to_string(), clean);
        }
    }

    Ok(data)
}

fn extract_between(html: &str, start_marker: &str, end_tag: &str) -> Option<String> {
    let start = html.find(start_marker)?;
    let after = &html[start..];
    // Find the next > to skip past the opening tag
    let content_start = after.find('>')? + 1;
    let content = &after[content_start..];
    let end = content.find(end_tag)?;
    Some(content[..end].to_string())
}

fn strip_html(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _   => if !in_tag { out.push(c); }
        }
    }
    // Collapse whitespace
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ── Cover image fetching ──────────────────────────────────────────────────────

fn fetch_image_b64(url: &str) -> Result<String, String> {
    // Skip data: URLs and empty
    if url.is_empty() || url.starts_with("data:") { return Err("invalid url".to_string()); }

    let bytes = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent("Mozilla/5.0")
        .build()
        .unwrap()
        .get(url)
        .send()
        .map_err(|e| e.to_string())?
        .bytes()
        .map_err(|e| e.to_string())?;

    use base64::Engine;
    Ok(base64::engine::general_purpose::STANDARD.encode(&bytes))
}

// ── AI analysis ───────────────────────────────────────────────────────────────

fn call_competition_ai(
    api_key: &str,
    model: &str,
    books: &[CompetitorBook],
    covers: &[(String, String)], // (title, base64_jpeg)
    genre_context: &str,
    keywords: &[String],
) -> Result<String, String> {

    // Build compact text summary of all books
    let mut book_summary = String::new();
    for (i, b) in books.iter().take(20).enumerate() {
        book_summary.push_str(&format!(
            "{}. \"{}\" {}{} by {} | Rank: {} | Reviews: {} ({}) | Price: ${} | Sales/mo: {}\n",
            i + 1,
            b.title,
            if b.subtitle.is_empty() { String::new() } else { format!(": {}", b.subtitle) },
            if b.series.is_empty() { String::new() } else { format!(" [{}]", b.series) },
            b.author,
            b.rank,
            b.reviews,
            b.rating,
            b.price,
            b.monthly_sales,
        ));
        if !b.description.is_empty() {
            book_summary.push_str(&format!("   Blurb: {}\n", &b.description[..b.description.len().min(200)]));
        }
    }

    let system = r#"You are a book marketing expert helping an indie author compete in their genre.

You will receive:
- The book's genre context
- Data on 10-20 competing books (titles, subtitles, series, rankings, reviews, pricing, descriptions)
- Cover images of the top competitors

Produce a BRIEF competitive positioning report. Be specific and direct. No padding.

Return ONLY a JSON object with this exact structure:
{
  "cover": "2-3 bullet points on cover design patterns and what to do/avoid",
  "title": "2-3 bullet points on title conventions and power words",
  "subtitle": "2-3 bullet points on subtitle formula and examples",
  "series": "1-2 bullet points on series naming patterns",
  "pricing": "ebook and print sweet spots with specific numbers",
  "positioning": "2-3 bullet points on how to position this book against competition",
  "top_comps": ["Book Title by Author — one sentence on why it matters", ...up to 3],
  "summary": "One paragraph synthesis — the single most important thing to get right"
}"#;

    let user_text = format!(
        "Genre context:\n{}\n\nKeywords analyzed: {}\n\nCompeting books:\n{}",
        if genre_context.len() > 1000 { &genre_context[..1000] } else { genre_context },
        keywords.join(", "),
        book_summary
    );

    // Build message with covers if available
    if covers.is_empty() {
        // Text-only call
        let raw = call_anthropic(api_key, model, system, &user_text, 1000)?;
        return build_report_from_json(&raw, books, keywords);
    }

    // Vision call with cover images
    let mut content_parts = vec![
        json!({"type": "text", "text": user_text}),
    ];

    for (title, b64) in covers.iter().take(8) {
        content_parts.push(json!({
            "type": "text",
            "text": format!("Cover image for: {}", title)
        }));
        content_parts.push(json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": "image/jpeg",
                "data": b64
            }
        }));
    }

    let body = json!({
        "model": model,
        "max_tokens": 1000,
        "system": system,
        "messages": [{"role": "user", "content": content_parts}]
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

    let json_resp: Value = resp.json()
        .map_err(|e| format!("Anthropic response parse failed: {}", e))?;

    if let Some(err) = json_resp.get("error") {
        return Err(format!("Anthropic API error: {}",
            err["message"].as_str().unwrap_or("unknown")));
    }

    let raw = json_resp["content"][0]["text"]
        .as_str()
        .ok_or("Empty response from Anthropic")?
        .to_string();

    build_report_from_json(&raw, books, keywords)
}

fn build_report_from_json(
    raw: &str,
    books: &[CompetitorBook],
    keywords: &[String],
) -> Result<String, String> {
    let clean = raw.trim()
        .trim_start_matches("```json").trim_start_matches("```")
        .trim_end_matches("```").trim();

    let v: Value = serde_json::from_str(clean)
        .map_err(|e| format!("JSON parse error: {} | got: {}", e, &clean[..clean.len().min(300)]))?;

    let str_field = |key: &str| v[key].as_str().unwrap_or("").to_string();

    let now = chrono::Utc::now().format("%B %-d, %Y %H:%M UTC").to_string();

    let mut md = vec![
        "# Competition Analysis".to_string(),
        format!("Generated: {}", now),
        format!("Keywords analyzed: {}", keywords.len()),
        format!("Books analyzed: {}", books.len().min(20)),
        String::new(),
        "---".to_string(),
        String::new(),
    ];

    // Summary first — most important thing
    let summary = str_field("summary");
    if !summary.is_empty() {
        md.push(format!("> {}", summary));
        md.push(String::new());
        md.push("---".to_string());
        md.push(String::new());
    }

    // Top comps
    if let Some(comps) = v["top_comps"].as_array() {
        if !comps.is_empty() {
            md.push("## Study These Books".to_string());
            md.push(String::new());
            for c in comps {
                if let Some(s) = c.as_str() {
                    md.push(format!("- {}", s));
                }
            }
            md.push(String::new());
        }
    }

    for (section, label) in &[
        ("cover",        "## Cover Design"),
        ("title",        "## Title"),
        ("subtitle",     "## Subtitle"),
        ("series",       "## Series"),
        ("pricing",      "## Pricing"),
        ("positioning",  "## Positioning"),
    ] {
        let text = str_field(section);
        if !text.is_empty() {
            md.push(label.to_string());
            md.push(String::new());
            // Each bullet on its own line
            for line in text.lines() {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    if trimmed.starts_with('-') || trimmed.starts_with('•') {
                        md.push(trimmed.to_string());
                    } else {
                        md.push(format!("- {}", trimmed));
                    }
                }
            }
            md.push(String::new());
        }
    }

    Ok(md.join("\n"))
}

// ── Keyword loader ────────────────────────────────────────────────────────────

fn load_keywords(analysis_dir: &Path) -> Vec<String> {
    // Use pr-keywords.json — short phrases specifically for PR Competition Analyzer.
    // These are generated by generate_pr_keywords() and are completely separate
    // from the KDP keyword strings in kdp-keywords.md.
    if let Ok(text) = fs::read_to_string(analysis_dir.join("pr-keywords.json")) {
        if let Ok(keywords) = serde_json::from_str::<Vec<String>>(&text) {
            if !keywords.is_empty() {
                return keywords;
            }
        }
    }

    Vec::new()
}

fn err(msg: &str) -> CompetitionResult {
    CompetitionResult { success: false, report: String::new(), error: msg.to_string() }
}
