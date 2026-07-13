// canopy.rs — Canopy API client for Amazon book data.
//
// Base URL: https://rest.canopyapi.co
// Auth: API-KEY header
// Endpoints used:
//   /api/amazon/product         — product info by ASIN
//   /api/amazon/product/sales   — sales estimates by ASIN
//   /api/amazon/bestsellers     — top books in a category by node ID
//   /api/amazon/search          — search products by keyword
//   /api/amazon/autocomplete    — keyword suggestions

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

const BASE_URL: &str = "https://rest.canopyapi.co";
const TIMEOUT_SECS: u64 = 30;

// ── Client ────────────────────────────────────────────────────────────────────

pub struct CanopyClient {
    client: Client,
    api_key: String,
}

impl CanopyClient {
    pub fn new(api_key: &str) -> Result<Self, String> {
        let client = Client::builder()
            .timeout(Duration::from_secs(TIMEOUT_SECS))
            .build()
            .map_err(|e| format!("HTTP client error: {}", e))?;
        Ok(Self { client, api_key: api_key.to_string() })
    }

    fn get(&self, path: &str, params: &[(&str, &str)]) -> Result<Value, String> {
        let url = format!("{}{}", BASE_URL, path);
        let resp = self.client.get(&url)
            .header("API-KEY", &self.api_key)
            .query(params)
            .send()
            .map_err(|e| format!("Canopy request failed: {}", e))?;

        let status = resp.status();
        let body = resp.text().map_err(|e| format!("Canopy read error: {}", e))?;

        if !status.is_success() {
            return Err(format!("Canopy API error ({}): {}", status.as_u16(), &body[..body.len().min(300)]));
        }

        serde_json::from_str(&body)
            .map_err(|e| format!("Canopy JSON parse error: {} | body: {}", e, &body[..body.len().min(200)]))
    }
}

// ── Product ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductInfo {
    pub asin: String,
    pub title: String,
    pub author: Option<String>,
    pub price: Option<f64>,
    pub rating: Option<f64>,
    pub review_count: Option<u32>,
    pub bsr: Option<u64>,
    pub categories: Vec<String>,
    pub publisher: Option<String>,
    pub page_count: Option<u32>,
    pub kindle_unlimited: bool,
    pub image_url: Option<String>,
}

impl CanopyClient {
    pub fn get_product(&self, asin: &str, domain: &str) -> Result<ProductInfo, String> {
        let resp = self.get("/api/amazon/product", &[("asin", asin), ("domain", domain)])?;
        parse_product(&resp)
    }
}

fn parse_product(v: &Value) -> Result<ProductInfo, String> {
    // Canopy nests product data — try common paths
    let data = if v.get("data").is_some() { &v["data"] } else { v };
    let product = if data.get("product").is_some() { &data["product"] } else { data };

    Ok(ProductInfo {
        asin: product["asin"].as_str().unwrap_or("").to_string(),
        title: product["title"].as_str().unwrap_or("").to_string(),
        author: product["author"].as_str().or_else(|| product["brand"].as_str()).map(|s| s.to_string()),
        price: product["price"].as_f64().or_else(|| product["currentPrice"].as_f64()),
        rating: product["rating"].as_f64().or_else(|| product["stars"].as_f64()),
        review_count: product["reviewCount"].as_u64().or_else(|| product["totalReviews"].as_u64()).map(|n| n as u32),
        bsr: product["bsr"].as_u64()
            .or_else(|| product["salesRank"].as_u64())
            .or_else(|| product["bestSellersRank"].as_u64()),
        categories: extract_string_array(&product["categories"])
            .or_else(|| extract_string_array(&product["breadcrumbs"]))
            .unwrap_or_default(),
        publisher: product["publisher"].as_str().or_else(|| product["manufacturer"].as_str()).map(|s| s.to_string()),
        page_count: product["pageCount"].as_u64().or_else(|| product["pages"].as_u64()).map(|n| n as u32),
        kindle_unlimited: product["kindleUnlimited"].as_bool()
            .or_else(|| product["isKindleUnlimited"].as_bool())
            .unwrap_or(false),
        image_url: product["imageUrl"].as_str().or_else(|| product["mainImage"].as_str()).map(|s| s.to_string()),
    })
}

// ── Sales Estimate ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SalesEstimate {
    pub asin: String,
    pub estimated_daily_sales: Option<f64>,
    pub estimated_monthly_sales: Option<f64>,
    pub bsr: Option<u64>,
}

impl CanopyClient {
    pub fn get_sales(&self, asin: &str, domain: &str) -> Result<SalesEstimate, String> {
        let resp = self.get("/api/amazon/product/sales", &[("asin", asin), ("domain", domain)])?;
        let data = if resp.get("data").is_some() { &resp["data"] } else { &resp };

        Ok(SalesEstimate {
            asin: asin.to_string(),
            estimated_daily_sales: data["estimatedDailySales"].as_f64()
                .or_else(|| data["dailySales"].as_f64())
                .or_else(|| data["salesEstimate"].as_f64()),
            estimated_monthly_sales: data["estimatedMonthlySales"].as_f64()
                .or_else(|| data["monthlySales"].as_f64()),
            bsr: data["bsr"].as_u64().or_else(|| data["salesRank"].as_u64()),
        })
    }
}

// ── Bestsellers ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BestsellerEntry {
    pub rank: u32,
    pub asin: String,
    pub title: String,
    pub author: Option<String>,
    pub price: Option<f64>,
    pub rating: Option<f64>,
    pub review_count: Option<u32>,
    pub image_url: Option<String>,
}

impl CanopyClient {
    pub fn get_bestsellers(&self, category_id: &str, domain: &str, page: u32) -> Result<Vec<BestsellerEntry>, String> {
        let page_str = page.to_string();
        let resp = self.get("/api/amazon/bestsellers", &[
            ("categoryId", category_id), ("domain", domain), ("page", &page_str),
        ])?;

        let items = if resp.get("data").is_some() {
            &resp["data"]
        } else if resp.get("products").is_some() {
            &resp["products"]
        } else if resp.get("items").is_some() {
            &resp["items"]
        } else {
            &resp
        };

        let arr = items.as_array().ok_or("Bestsellers response is not an array")?;
        let mut entries = Vec::new();

        for (i, item) in arr.iter().enumerate() {
            entries.push(BestsellerEntry {
                rank: item["rank"].as_u64().unwrap_or((i + 1) as u64) as u32,
                asin: item["asin"].as_str().unwrap_or("").to_string(),
                title: item["title"].as_str().unwrap_or("").to_string(),
                author: item["author"].as_str().or_else(|| item["brand"].as_str()).map(|s| s.to_string()),
                price: item["price"].as_f64().or_else(|| item["currentPrice"].as_f64()),
                rating: item["rating"].as_f64().or_else(|| item["stars"].as_f64()),
                review_count: item["reviewCount"].as_u64().or_else(|| item["totalReviews"].as_u64()).map(|n| n as u32),
                image_url: item["imageUrl"].as_str().or_else(|| item["mainImage"].as_str()).map(|s| s.to_string()),
            });
        }
        Ok(entries)
    }
}

// ── Search ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub position: u32,
    pub asin: String,
    pub title: String,
    pub author: Option<String>,
    pub price: Option<f64>,
    pub rating: Option<f64>,
    pub review_count: Option<u32>,
    pub is_sponsored: bool,
    pub image_url: Option<String>,
}

impl CanopyClient {
    pub fn search(&self, term: &str, domain: &str, category_id: Option<&str>, page: u32) -> Result<Vec<SearchResult>, String> {
        let page_str = page.to_string();
        let mut params: Vec<(&str, &str)> = vec![
            ("searchTerm", term), ("domain", domain), ("page", &page_str),
        ];
        if let Some(cat) = category_id {
            params.push(("categoryId", cat));
        }

        let resp = self.get("/api/amazon/search", &params)?;
        let items = if resp.get("data").is_some() {
            &resp["data"]
        } else if resp.get("products").is_some() {
            &resp["products"]
        } else if resp.get("results").is_some() {
            &resp["results"]
        } else {
            &resp
        };

        let arr = items.as_array().ok_or("Search response is not an array")?;
        let mut results = Vec::new();

        for (i, item) in arr.iter().enumerate() {
            results.push(SearchResult {
                position: item["position"].as_u64().unwrap_or((i + 1) as u64) as u32,
                asin: item["asin"].as_str().unwrap_or("").to_string(),
                title: item["title"].as_str().unwrap_or("").to_string(),
                author: item["author"].as_str().or_else(|| item["brand"].as_str()).map(|s| s.to_string()),
                price: item["price"].as_f64().or_else(|| item["currentPrice"].as_f64()),
                rating: item["rating"].as_f64().or_else(|| item["stars"].as_f64()),
                review_count: item["reviewCount"].as_u64().or_else(|| item["totalReviews"].as_u64()).map(|n| n as u32),
                is_sponsored: item["isSponsored"].as_bool().or_else(|| item["sponsored"].as_bool()).unwrap_or(false),
                image_url: item["imageUrl"].as_str().or_else(|| item["mainImage"].as_str()).map(|s| s.to_string()),
            });
        }
        Ok(results)
    }
}

// ── Autocomplete ──────────────────────────────────────────────────────────────

impl CanopyClient {
    pub fn autocomplete(&self, term: &str, domain: &str, category: Option<&str>) -> Result<Vec<String>, String> {
        let mut params: Vec<(&str, &str)> = vec![
            ("searchTerm", term), ("domain", domain),
        ];
        if let Some(cat) = category {
            params.push(("category", cat));
        }

        let resp = self.get("/api/amazon/autocomplete", &params)?;
        let suggestions = if resp.get("data").is_some() {
            &resp["data"]
        } else if resp.get("suggestions").is_some() {
            &resp["suggestions"]
        } else {
            &resp
        };

        match suggestions.as_array() {
            Some(arr) => Ok(arr.iter().filter_map(|v| {
                v.as_str().map(|s| s.to_string())
                    .or_else(|| v["value"].as_str().map(|s| s.to_string()))
                    .or_else(|| v["suggestion"].as_str().map(|s| s.to_string()))
            }).collect()),
            None => Ok(Vec::new()),
        }
    }

    /// Simple connection test — do a lightweight autocomplete call.
    pub fn test_connection(&self) -> Result<(), String> {
        self.autocomplete("book", "amazon.com", None)?;
        Ok(())
    }
}

// ── Category Stats (derived) ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryStats {
    pub sales_to_one: String,
    pub sales_to_ten: String,
    pub publisher_pct: String,
    pub ku_pct: String,
}

impl CanopyClient {
    /// Get competition stats for a category using its Amazon node ID.
    /// Pulls the bestseller list, then estimates sales for rank 1 and 10,
    /// and calculates publisher% and KU% from the top 20 books.
    pub fn category_stats(&self, node_id: &str, domain: &str) -> Result<CategoryStats, String> {
        let bestsellers = self.get_bestsellers(node_id, domain, 1)?;
        if bestsellers.is_empty() {
            return Ok(CategoryStats {
                sales_to_one: "N/A".to_string(),
                sales_to_ten: "N/A".to_string(),
                publisher_pct: "N/A".to_string(),
                ku_pct: "N/A".to_string(),
            });
        }

        // Get sales estimates for rank 1 and rank 10
        let sales_one = if let Some(b) = bestsellers.first() {
            match self.get_sales(&b.asin, domain) {
                Ok(s) => s.estimated_daily_sales.map(|d| format!("{:.0}/day", d)).unwrap_or_else(|| "N/A".to_string()),
                Err(_) => "N/A".to_string(),
            }
        } else { "N/A".to_string() };

        let sales_ten = if let Some(b) = bestsellers.get(9) {
            match self.get_sales(&b.asin, domain) {
                Ok(s) => s.estimated_daily_sales.map(|d| format!("{:.0}/day", d)).unwrap_or_else(|| "N/A".to_string()),
                Err(_) => "N/A".to_string(),
            }
        } else { "N/A".to_string() };

        // Calculate publisher % and KU % from available data
        // For publisher %, we need full product info — use the top 20
        let sample_size = bestsellers.len().min(20);
        let mut indie_count = 0u32;
        let mut ku_count = 0u32;
        let mut checked = 0u32;

        for entry in bestsellers.iter().take(sample_size) {
            if entry.asin.is_empty() { continue; }
            if let Ok(product) = self.get_product(&entry.asin, domain) {
                checked += 1;
                if let Some(pub_name) = &product.publisher {
                    let lower = pub_name.to_lowercase();
                    if lower.contains("independently published") || lower.contains("self-published") || lower.contains("kindle direct") {
                        indie_count += 1;
                    }
                }
                if product.kindle_unlimited { ku_count += 1; }
            }
            // Rate limit — don't hammer the API
            std::thread::sleep(Duration::from_millis(100));
        }

        let publisher_pct = if checked > 0 {
            let trad_pct = 100.0 * (checked - indie_count) as f64 / checked as f64;
            format!("{:.0}%", trad_pct)
        } else { "N/A".to_string() };

        let ku_pct = if checked > 0 {
            format!("{:.0}%", 100.0 * ku_count as f64 / checked as f64)
        } else { "N/A".to_string() };

        Ok(CategoryStats { sales_to_one: sales_one, sales_to_ten: sales_ten, publisher_pct, ku_pct })
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn extract_string_array(v: &Value) -> Option<Vec<String>> {
    v.as_array().map(|arr| {
        arr.iter().filter_map(|item| {
            item.as_str().map(|s| s.to_string())
                .or_else(|| item["name"].as_str().map(|s| s.to_string()))
        }).collect()
    })
}

// ── Tauri commands ────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct CanopyTestResult {
    pub success: bool,
    pub error: String,
}

#[tauri::command]
pub async fn test_canopy_connection(api_key: String) -> CanopyTestResult {
    match tokio::task::spawn_blocking(move || {
        let client = CanopyClient::new(&api_key)?;
        client.test_connection()
    }).await.unwrap() {
        Ok(()) => CanopyTestResult { success: true, error: String::new() },
        Err(e) => CanopyTestResult { success: false, error: e },
    }
}


// ── Tauri commands for category stats via Canopy ──────────────────────────────

use tauri::{AppHandle, Emitter, Manager};
use crate::db;
use crate::commands::CategoryStatRow;

fn emit_canopy(app: &AppHandle, msg: &str) { let _ = app.emit("cdp:log", msg); }

/// Map store name to Amazon domain for Canopy API
fn store_to_domain(store: &str) -> &str {
    match store {
        "Kindle" | "kindle" => "amazon.com",
        "Books" | "books" => "amazon.com",
        _ => "amazon.com",
    }
}

/// Canopy-based replacement for the PR analyze_categories command.
/// Takes category paths, looks up node IDs from DB, fetches stats via Canopy.
#[tauri::command]
pub async fn analyze_categories_canopy(
    app: AppHandle,
    paths: Vec<String>,
    store: String,
    canopy_api_key: String,
) -> crate::commands::AnalyzerResult {
    tokio::task::spawn_blocking(move || {
        let client = match CanopyClient::new(&canopy_api_key) {
            Ok(c) => c,
            Err(e) => return crate::commands::AnalyzerResult {
                success: false, markdown: String::new(), error: e, rows: Vec::new(),
            },
        };

        let database = app.state::<db::Db>();
        let domain = store_to_domain(&store);
        let mut rows: Vec<CategoryStatRow> = Vec::new();

        for (i, path) in paths.iter().enumerate() {
            emit_canopy(&app, &format!("[{}/{}] {}", i + 1, paths.len(), path));

            // Look up node ID from DB
            let node_id = {
                let conn = database.0.lock().unwrap();
                db::node_id_for_path(&conn, path, &store)
            };

            let node_id = match node_id {
                Some(id) => id,
                None => {
                    emit_canopy(&app, &format!("  ⚠ No node ID for this path — skipping."));
                    rows.push(CategoryStatRow {
                        requested_path: path.clone(),
                        matched_path: String::new(),
                        found: false,
                        sales_to_one: String::new(),
                        sales_to_ten: String::new(),
                        publisher_pct: String::new(),
                        ku_pct: String::new(),
                    });
                    continue;
                }
            };

            emit_canopy(&app, &format!("  Node ID: {} — fetching bestsellers...", node_id));

            match client.category_stats(&node_id, domain) {
                Ok(stats) => {
                    emit_canopy(&app, &format!("  ✓ #1={}, #10={}, Publisher={}, KU={}",
                        stats.sales_to_one, stats.sales_to_ten, stats.publisher_pct, stats.ku_pct));
                    rows.push(CategoryStatRow {
                        requested_path: path.clone(),
                        matched_path: path.clone(),
                        found: true,
                        sales_to_one: stats.sales_to_one,
                        sales_to_ten: stats.sales_to_ten,
                        publisher_pct: stats.publisher_pct,
                        ku_pct: stats.ku_pct,
                    });
                }
                Err(e) => {
                    emit_canopy(&app, &format!("  ⚠ Canopy error: {}", e));
                    rows.push(CategoryStatRow {
                        requested_path: path.clone(),
                        matched_path: String::new(),
                        found: false,
                        sales_to_one: String::new(),
                        sales_to_ten: String::new(),
                        publisher_pct: String::new(),
                        ku_pct: String::new(),
                    });
                }
            }

            // Small delay between categories to be polite to the API
            std::thread::sleep(std::time::Duration::from_millis(200));
        }

        let success = rows.iter().any(|r| r.found);
        crate::commands::AnalyzerResult {
            success,
            markdown: String::new(),
            error: if success { String::new() } else { "No categories could be verified via Canopy.".to_string() },
            rows,
        }
    }).await.unwrap()
}


// ── Competition Analysis via Canopy ───────────────────────────────────────────

use crate::competition_analyzer::{CompetitorBook, CompetitionResult, CompetitionData};
use crate::commands::call_llm;

#[derive(Deserialize)]
pub struct CompetitionCanopyRequest {
    pub folder:         String,
    pub api_key:        String,
    pub model:          String,
    pub store:          String,
    pub provider:       String,
    pub canopy_api_key: String,
}

#[tauri::command]
pub async fn analyze_competition_canopy(
    app: AppHandle,
    request: CompetitionCanopyRequest,
) -> CompetitionResult {
    tokio::task::spawn_blocking(move || {
        let database = app.state::<db::Db>();
        run_competition_canopy(&app, &database, &request)
    }).await.unwrap()
}

fn run_competition_canopy(app: &AppHandle, database: &db::Db, req: &CompetitionCanopyRequest) -> CompetitionResult {
    let client = match CanopyClient::new(&req.canopy_api_key) {
        Ok(c) => c,
        Err(e) => return CompetitionResult { success: false, report: String::new(), error: e },
    };

    let keywords = { let conn = database.0.lock().unwrap(); db::load_pr_keywords(&conn, &req.folder) };
    if keywords.is_empty() {
        return CompetitionResult { success: false, report: String::new(), error: "No PR search terms found. Run Analyze first.".to_string() };
    }

    let domain = store_to_domain(&req.store);
    emit_canopy(app, &format!("Competition analysis via Canopy API — {} keyword(s)", keywords.len()));

    let mut all_books: Vec<CompetitorBook> = Vec::new();
    let mut seen_asins: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (i, keyword) in keywords.iter().enumerate() {
        if crate::is_cancelled() { break; }
        emit_canopy(app, &format!("[{}/{}] Searching: \"{}\"", i + 1, keywords.len(), keyword));

        // Search for books with this keyword in Kindle store
        let category_id = if req.store == "Kindle" { Some("digital-text") } else { None };
        let search_results = match client.search(keyword, domain, category_id, 1) {
            Ok(r) => r,
            Err(e) => {
                emit_canopy(app, &format!("  ⚠ Search failed: {}", e));
                continue;
            }
        };

        emit_canopy(app, &format!("  {} results found.", search_results.len()));

        // Get details for top 10 non-sponsored results
        let organic: Vec<_> = search_results.iter().filter(|r| !r.is_sponsored).take(10).collect();
        for sr in &organic {
            if sr.asin.is_empty() || !seen_asins.insert(sr.asin.clone()) { continue; }

            // Get sales data
            let sales = client.get_sales(&sr.asin, domain).ok();
            let daily = sales.as_ref().and_then(|s| s.estimated_daily_sales).map(|d| format!("{:.0}", d)).unwrap_or_default();
            let monthly = sales.as_ref().and_then(|s| s.estimated_monthly_sales).map(|d| format!("{:.0}", d)).unwrap_or_default();
            let bsr = sales.as_ref().and_then(|s| s.bsr).map(|b| b.to_string()).unwrap_or_default();

            all_books.push(CompetitorBook {
                title:        sr.title.clone(),
                subtitle:     String::new(),
                review_score: sr.rating.map(|r| format!("{:.1}", r)).unwrap_or_default(),
                ratings:      sr.review_count.map(|r| r.to_string()).unwrap_or_default(),
                author:       sr.author.clone().unwrap_or_default(),
                age:          String::new(),
                absr:         bsr,
                pages:        String::new(),
                kwt:          String::new(),
                price:        sr.price.map(|p| format!("${:.2}", p)).unwrap_or_default(),
                dy_sales:     daily,
                mo_sales:     monthly,
                amazon_url:   format!("https://amazon.com/dp/{}", sr.asin),
                keyword:      keyword.clone(),
                cover_url:    sr.image_url.clone().unwrap_or_default(),
            });

            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    if all_books.is_empty() {
        return CompetitionResult { success: false, report: String::new(), error: "No books found for any keyword.".to_string() };
    }

    emit_canopy(app, &format!("Total: {} unique competitor books", all_books.len()));

    // Save raw data
    let folder = std::path::PathBuf::from(&req.folder);
    let analysis_dir = folder.join("_analysis");
    let _ = std::fs::create_dir_all(&analysis_dir);
    let data = CompetitionData {
        generated: chrono::Utc::now().to_rfc3339(),
        keywords_analyzed: keywords.clone(),
        books: all_books.clone(),
        categories: Vec::new(), // No category CSV from Canopy — we already have WinningCat
    };
    if let Ok(json) = serde_json::to_string_pretty(&data) {
        let _ = std::fs::write(analysis_dir.join("competition-data.json"), &json);
        emit_canopy(app, "  ✓ competition-data.json saved.");
    }

    // AI analysis
    emit_canopy(app, &format!("Running AI analysis... [{}]", req.model));
    let genre_context = {
        let conn = database.0.lock().unwrap();
        db::load_genre_data(&conn, &req.folder).map(|g| g.genre_signals).unwrap_or_default()
    };

    let books_summary: String = all_books.iter().take(20).enumerate().map(|(i, b)| {
        format!("{}. \"{}\" by {} — BSR: {}, Price: {}, Rating: {} ({} reviews), Daily sales: {}, Keyword: \"{}\"",
            i + 1, b.title, b.author, b.absr, b.price, b.review_score, b.ratings, b.dy_sales, b.keyword)
    }).collect::<Vec<_>>().join("\n");

    let system = r#"You are a publishing strategist. Analyze this competition data for an indie author. Produce a clear, actionable report covering:
1. Competition Summary — how competitive is this niche?
2. Key Books to Study — what are the top competitors doing right?
3. Pricing Analysis — what price points work?
4. What This Means for the Author's Book
5. Verdict — is this niche viable for a debut?

Be direct, concise, no filler."#;

    let user = format!(
        "Genre context: {}\n\nKeywords analyzed: {}\n\nTop competitor books:\n{}",
        genre_context, keywords.join(", "), books_summary
    );

    match call_llm(&req.provider, &req.api_key, &req.model, system, &user, 2000) {
        Err(e) => CompetitionResult { success: false, report: String::new(), error: format!("AI error: {}", e) },
        Ok(report) => {
            let json = serde_json::json!({
                "schema": "competition_report_v1",
                "content_format": "markdown",
                "content": report,
            }).to_string();
            let conn = database.0.lock().unwrap();
            let _ = db::save_document(&conn, &req.folder, "competition_report", &json);
            emit_canopy(app, "✓ Competition report saved to database.");
            CompetitionResult { success: true, report: json, error: String::new() }
        }
    }
}


// ── Keyword Search via Canopy (replaces PR Keyword Search) ────────────────────

use crate::keyword_search::{KeywordResult, KeywordSearchResponse};

#[derive(Deserialize)]
pub struct KeywordSearchCanopyRequest {
    pub folder:         String,
    pub seed:           String,
    pub canopy_api_key: String,
}

#[tauri::command]
pub async fn search_keywords_canopy(app: AppHandle, request: KeywordSearchCanopyRequest) -> KeywordSearchResponse {
    tokio::task::spawn_blocking(move || {
        let client = match CanopyClient::new(&request.canopy_api_key) {
            Ok(c) => c,
            Err(e) => return KeywordSearchResponse { success: false, results: Vec::new(), error: e },
        };

        emit_canopy(&app, &format!("Keyword search via Canopy: \"{}\"", request.seed));

        // Step 1: Get autocomplete suggestions
        let suggestions = match client.autocomplete(&request.seed, "amazon.com", Some("digital-text")) {
            Ok(s) => s,
            Err(e) => {
                emit_canopy(&app, &format!("  ⚠ Autocomplete failed: {}", e));
                vec![request.seed.clone()] // Fall back to just the seed
            }
        };

        // Include the original seed + up to 15 suggestions
        let mut keywords_to_check: Vec<String> = vec![request.seed.clone()];
        for s in suggestions.into_iter().take(15) {
            if !keywords_to_check.contains(&s) {
                keywords_to_check.push(s);
            }
        }
        emit_canopy(&app, &format!("  {} keywords to analyze.", keywords_to_check.len()));

        let mut results: Vec<KeywordResult> = Vec::new();

        for (i, keyword) in keywords_to_check.iter().enumerate() {
            if crate::is_cancelled() { break; }
            emit_canopy(&app, &format!("  [{}/{}] \"{}\"", i + 1, keywords_to_check.len(), keyword));

            // Step 2: Search for this keyword in Kindle store
            let search_results = match client.search(keyword, "amazon.com", Some("digital-text"), 1) {
                Ok(r) => r,
                Err(_) => { continue; }
            };

            if search_results.is_empty() {
                results.push(KeywordResult {
                    keyword: keyword.clone(),
                    searches: "0".to_string(),
                    competition: "Low".to_string(),
                    estimated_earnings: "$0".to_string(),
                });
                continue;
            }

            // Step 3: Estimate search volume from top results' sales
            // Logic: if the #1 book for this keyword sells X/day, and typical
            // click-through rate for position 1 is ~30%, and conversion is ~3%,
            // then monthly searches ≈ X * 30 / 0.30 / 0.03 ≈ X * 3333
            // We use a more conservative multiplier of 1000 based on industry data.
            let organic: Vec<_> = search_results.iter().filter(|r| !r.is_sponsored).take(3).collect();

            let mut top_daily_sales: Vec<f64> = Vec::new();
            for sr in &organic {
                if sr.asin.is_empty() { continue; }
                if let Ok(sales) = client.get_sales(&sr.asin, "amazon.com") {
                    if let Some(daily) = sales.estimated_daily_sales {
                        top_daily_sales.push(daily);
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }

            let avg_daily = if top_daily_sales.is_empty() { 0.0 }
                else { top_daily_sales.iter().sum::<f64>() / top_daily_sales.len() as f64 };

            // Volume estimation: avg daily sales of top 3 × multiplier
            // Conservative model: each sale represents ~30 searches (3% CTR × 3% conversion)
            let estimated_monthly_searches = (avg_daily * 30.0 * 33.0) as u64;

            // Step 4: Competition score based on review counts of page 1 results
            let avg_reviews: f64 = {
                let review_counts: Vec<f64> = search_results.iter()
                    .filter_map(|r| r.review_count.map(|c| c as f64))
                    .collect();
                if review_counts.is_empty() { 0.0 }
                else { review_counts.iter().sum::<f64>() / review_counts.len() as f64 }
            };
            let sponsored_count = search_results.iter().filter(|r| r.is_sponsored).count();

            let competition = if avg_reviews > 500.0 || sponsored_count > 5 { "High" }
                else if avg_reviews > 100.0 || sponsored_count > 2 { "Medium" }
                else { "Low" };

            // Step 5: Estimated earnings (royalty if you ranked #1)
            // Avg Kindle royalty ≈ $2.80 per sale, top position gets ~30% of clicks
            let est_monthly_sales = avg_daily * 30.0 * 0.3;
            let est_earnings = est_monthly_sales * 2.80;

            results.push(KeywordResult {
                keyword: keyword.clone(),
                searches: format!("{}", estimated_monthly_searches),
                competition: competition.to_string(),
                estimated_earnings: format!("${:.0}", est_earnings),
            });
        }

        emit_canopy(&app, &format!("✓ {} keyword(s) analyzed.", results.len()));

        // Save to database
        let database = app.state::<db::Db>();
        let conn = database.0.lock().unwrap();
        let rows: Vec<(String, String, String, String)> = results.iter()
            .map(|r| (r.keyword.clone(), r.searches.clone(), r.competition.clone(), r.estimated_earnings.clone()))
            .collect();
        let _ = db::replace_keyword_search_results(&conn, &request.folder, &request.seed, &rows);

        KeywordSearchResponse { success: true, results, error: String::new() }
    }).await.unwrap()
}
