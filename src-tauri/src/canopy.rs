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

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;
use tokio::time::sleep;

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

    async fn get(&self, path: &str, params: &[(&str, &str)]) -> Result<Value, String> {
        let url = format!("{}{}", BASE_URL, path);
        let resp = self.client.get(&url)
            .header("API-KEY", &self.api_key)
            .query(params)
            .send()
            .await
            .map_err(|e| format!("Canopy request failed: {}", e))?;

        let status = resp.status();
        let body = resp.text().await.map_err(|e| format!("Canopy read error: {}", e))?;

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
    pub async fn get_product(&self, asin: &str, domain: &str) -> Result<ProductInfo, String> {
        let resp = self.get("/api/amazon/product", &[("asin", asin), ("domain", domain)]).await?;
        parse_product(&resp)
    }
}

fn parse_product(v: &Value) -> Result<ProductInfo, String> {
    let product = &v["data"]["amazonProduct"];
    if product.is_null() {
        return Err("No amazonProduct in response".to_string());
    }

    Ok(ProductInfo {
        asin: product["asin"].as_str().unwrap_or("").to_string(),
        title: product["title"].as_str().unwrap_or("").to_string(),
        author: product["brand"].as_str().map(|s| s.to_string()),
        price: product["price"]["value"].as_f64(),
        rating: product["rating"].as_f64(),
        review_count: product["ratingsTotal"].as_u64().map(|n| n as u32),
        bsr: None, // BSR not in product endpoint — use sales endpoint
        categories: product["categories"].as_array()
            .map(|arr| arr.iter().filter_map(|c| c["name"].as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default(),
        publisher: product["technicalSpecifications"].as_array()
            .and_then(|specs| specs.iter().find(|s| s["name"].as_str() == Some("Publisher")))
            .and_then(|s| s["value"].as_str())
            .map(|s| s.to_string()),
        page_count: product["technicalSpecifications"].as_array()
            .and_then(|specs| specs.iter().find(|s| {
                let name = s["name"].as_str().unwrap_or("");
                name == "Print length" || name == "Pages"
            }))
            .and_then(|s| s["value"].as_str())
            .and_then(|v| v.replace(" pages", "").trim().parse::<u32>().ok()),
        kindle_unlimited: product["isKindleUnlimited"].as_bool().unwrap_or(false),
        image_url: product["mainImageUrl"].as_str().map(|s| s.to_string()),
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
    pub async fn get_sales(&self, asin: &str, domain: &str) -> Result<SalesEstimate, String> {
        let resp = self.get("/api/amazon/product/sales", &[("asin", asin), ("domain", domain)]).await?;
        let estimate = &resp["data"]["amazonProduct"]["salesEstimate"];

        let weekly = estimate["weeklyUnitSales"].as_f64();
        let monthly = estimate["monthlyUnitSales"].as_f64();
        let daily = weekly.map(|w| w / 7.0).or_else(|| monthly.map(|m| m / 30.0));

        Ok(SalesEstimate {
            asin: asin.to_string(),
            estimated_daily_sales: daily,
            estimated_monthly_sales: monthly,
            bsr: None, // BSR not returned by sales endpoint
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
    pub async fn get_bestsellers(&self, category_id: &str, domain: &str, page: u32) -> Result<Vec<BestsellerEntry>, String> {
        let page_str = page.to_string();
        let resp = self.get("/api/amazon/bestsellers", &[
            ("categoryId", category_id), ("domain", domain), ("page", &page_str),
        ]).await?;

        let results = &resp["data"]["amazonBestSellers"]["productResults"]["results"];
        let arr = results.as_array().ok_or("No bestseller results in response")?;
        let mut entries = Vec::new();

        for (i, item) in arr.iter().enumerate() {
            entries.push(BestsellerEntry {
                rank: item["bestSellersRank"].as_u64().unwrap_or((i + 1) as u64) as u32,
                asin: item["asin"].as_str().unwrap_or("").to_string(),
                title: item["title"].as_str().unwrap_or("").to_string(),
                author: None, // Not in bestseller results — need product lookup
                price: item["price"]["value"].as_f64(),
                rating: item["rating"].as_f64(),
                review_count: item["ratingsTotal"].as_u64().map(|n| n as u32),
                image_url: item["mainImageUrl"].as_str().map(|s| s.to_string()),
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
    pub async fn search(&self, term: &str, domain: &str, category_id: Option<&str>, page: u32) -> Result<Vec<SearchResult>, String> {
        let page_str = page.to_string();
        let mut params: Vec<(&str, &str)> = vec![
            ("searchTerm", term), ("domain", domain), ("page", &page_str),
        ];
        if let Some(cat) = category_id {
            params.push(("categoryId", cat));
        }

        let resp = self.get("/api/amazon/search", &params).await?;
        let results = &resp["data"]["amazonProductSearchResults"]["productResults"]["results"];
        let arr = results.as_array().ok_or("No search results in response")?;
        let mut out = Vec::new();

        for (i, item) in arr.iter().enumerate() {
            out.push(SearchResult {
                position: (i + 1) as u32,
                asin: item["asin"].as_str().unwrap_or("").to_string(),
                title: item["title"].as_str().unwrap_or("").to_string(),
                author: None, // Not directly in search results
                price: item["price"]["value"].as_f64(),
                rating: item["rating"].as_f64(),
                review_count: item["ratingsTotal"].as_u64().map(|n| n as u32),
                is_sponsored: item["sponsored"].as_bool().unwrap_or(false),
                image_url: item["mainImageUrl"].as_str().map(|s| s.to_string()),
            });
        }
        Ok(out)
    }
}

// ── Autocomplete ──────────────────────────────────────────────────────────────

impl CanopyClient {
    pub async fn autocomplete(&self, term: &str, domain: &str, category: Option<&str>) -> Result<Vec<String>, String> {
        let mut params: Vec<(&str, &str)> = vec![
            ("searchTerm", term), ("domain", domain),
        ];
        if let Some(cat) = category {
            params.push(("category", cat));
        }

        let resp = self.get("/api/amazon/autocomplete", &params).await?;
        let suggestions = &resp["data"]["amazonSearchAutocompleteResults"];

        match suggestions.as_array() {
            Some(arr) => Ok(arr.iter().filter_map(|v| {
                v["suggestion"].as_str().map(|s| s.to_string())
            }).collect()),
            None => Ok(Vec::new()),
        }
    }

    /// Simple connection test — do a lightweight autocomplete call.
    pub async fn test_connection(&self) -> Result<(), String> {
        self.autocomplete("book", "US", None).await?;
        Ok(())
    }
}

// ── Reviews ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewEntry {
    pub rating: u8,
    pub title: String,
    pub body: String,
    pub author: String,
    pub verified: bool,
    pub date: String,
}

impl CanopyClient {
    /// Fetch reviews for a product. Returns paginated reviews filtered by rating.
    pub async fn get_reviews(&self, asin: &str, domain: &str, page: u32, rating: Option<u8>) -> Result<Vec<ReviewEntry>, String> {
        let page_str = page.to_string();
        let mut params: Vec<(&str, &str)> = vec![
            ("asin", asin), ("domain", domain), ("page", &page_str),
        ];
        let rating_str;
        if let Some(r) = rating {
            rating_str = match r {
                5 => "FIVE_STAR",
                4 => "FOUR_STAR",
                3 => "THREE_STAR",
                2 => "TWO_STAR",
                1 => "ONE_STAR",
                _ => "ALL",
            }.to_string();
            params.push(("rating", &rating_str));
        }

        let resp = self.get("/api/amazon/product/reviews", &params).await?;
        let product = &resp["data"]["amazonProduct"];

        // Try paginated reviews first, fall back to topReviews
        let reviews_arr = if let Some(arr) = product["reviewsPaginated"]["reviews"].as_array() {
            arr
        } else if let Some(arr) = product["topReviews"].as_array() {
            arr
        } else {
            return Ok(Vec::new());
        };

        let mut entries = Vec::new();
        for item in reviews_arr {
            entries.push(ReviewEntry {
                rating: item["rating"].as_u64().unwrap_or(0) as u8,
                title: item["title"].as_str().unwrap_or("").to_string(),
                body: item["body"].as_str().unwrap_or("").to_string(),
                author: item["reviewer"]["name"].as_str().unwrap_or("").to_string(),
                verified: item["verifiedPurchase"].as_bool().unwrap_or(false),
                date: String::new(), // Not in spec response
            });
        }
        Ok(entries)
    }
}

// ── Author ────────────────────────────────────────────────────────────────────

// ── Category Products ─────────────────────────────────────────────────────────

impl CanopyClient {
    /// Fetch products listed in a specific category (paginated, deeper than bestsellers).
    /// Also returns subcategories that can be drilled into.
    pub async fn get_category_products(&self, category_id: &str, domain: &str, page: u32) -> Result<Vec<BestsellerEntry>, String> {
        let page_str = page.to_string();
        let resp = self.get("/api/amazon/category", &[
            ("categoryId", category_id), ("domain", domain), ("page", &page_str),
        ]).await?;

        let results = &resp["data"]["amazonProductCategory"]["productResults"]["results"];
        let arr = results.as_array().ok_or("No category product results in response")?;
        let mut entries = Vec::new();
        for (i, item) in arr.iter().enumerate() {
            entries.push(BestsellerEntry {
                rank: (i + 1) as u32,
                asin: item["asin"].as_str().unwrap_or("").to_string(),
                title: item["title"].as_str().unwrap_or("").to_string(),
                author: None,
                price: item["price"]["value"].as_f64(),
                rating: item["rating"].as_f64(),
                review_count: item["ratingsTotal"].as_u64().map(|n| n as u32),
                image_url: item["mainImageUrl"].as_str().map(|s| s.to_string()),
            });
        }
        Ok(entries)
    }
}

// ── Category Stats (derived) ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopBook {
    pub title: String,
    pub asin: String,
    pub image_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryStats {
    pub sales_to_one: String,
    pub sales_to_ten: String,
    pub publisher_pct: String,
    pub ku_pct: String,
    pub top_books: Vec<TopBook>,
}

impl CanopyClient {
    /// Get competition stats for a category using its Amazon node ID.
    /// Pulls the bestseller list, then estimates sales for rank 1 and 10,
    /// and calculates publisher% and KU% from the top 20 books.
    pub async fn category_stats(&self, node_id: &str, domain: &str) -> Result<CategoryStats, String> {
        let bestsellers = self.get_bestsellers(node_id, domain, 1).await?;
        if bestsellers.is_empty() {
            return Ok(CategoryStats {
                sales_to_one: "N/A".to_string(),
                sales_to_ten: "N/A".to_string(),
                publisher_pct: "N/A".to_string(),
                ku_pct: "N/A".to_string(),
                top_books: Vec::new(),
            });
        }

        // Capture top 3 books for the report
        let top_books: Vec<TopBook> = bestsellers.iter().take(3).map(|b| TopBook {
            title: b.title.clone(),
            asin: b.asin.clone(),
            image_url: b.image_url.clone(),
        }).collect();

        // Get sales estimates for rank 1 and rank 10
        let sales_one = if let Some(b) = bestsellers.first() {
            match self.get_sales(&b.asin, domain).await {
                Ok(s) => s.estimated_daily_sales.map(|d| format!("{:.0}/day", d)).unwrap_or_else(|| "N/A".to_string()),
                Err(_) => "N/A".to_string(),
            }
        } else { "N/A".to_string() };

        let sales_ten = if let Some(b) = bestsellers.get(9) {
            match self.get_sales(&b.asin, domain).await {
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
            if let Ok(product) = self.get_product(&entry.asin, domain).await {
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
            sleep(Duration::from_millis(100)).await;
        }

        let publisher_pct = if checked > 0 {
            let trad_pct = 100.0 * (checked - indie_count) as f64 / checked as f64;
            format!("{:.0}%", trad_pct)
        } else { "N/A".to_string() };

        let ku_pct = if checked > 0 {
            format!("{:.0}%", 100.0 * ku_count as f64 / checked as f64)
        } else { "N/A".to_string() };

        Ok(CategoryStats { sales_to_one: sales_one, sales_to_ten: sales_ten, publisher_pct, ku_pct, top_books })
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

// ── Tauri commands ────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct CanopyTestResult {
    pub success: bool,
    pub error: String,
}

#[tauri::command]
pub async fn test_canopy_connection(api_key: String) -> CanopyTestResult {
    let client = match CanopyClient::new(&api_key) {
        Ok(c) => c,
        Err(e) => return CanopyTestResult { success: false, error: e },
    };
    match client.test_connection().await {
        Ok(()) => CanopyTestResult { success: true, error: String::new() },
        Err(e) => CanopyTestResult { success: false, error: e },
    }
}


// ── Tauri commands for category stats via Canopy ──────────────────────────────

use tauri::{AppHandle, Emitter, Manager};
use crate::db;
use crate::commands::CategoryStatRow;

fn emit_canopy(app: &AppHandle, msg: &str) { let _ = app.emit("cdp:log", msg); }

/// Map store name to Canopy domain code
fn store_to_domain(store: &str) -> &str {
    match store {
        "Kindle" | "kindle" => "US",
        "Books" | "books" => "US",
        _ => "US",
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
                    top_books: Vec::new(),
                });
                continue;
            }
        };

        emit_canopy(&app, &format!("  Node ID: {} — fetching bestsellers...", node_id));

        match client.category_stats(&node_id, domain).await {
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
                    top_books: stats.top_books,
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
                    top_books: Vec::new(),
                });
            }
        }

        // Small delay between categories to be polite to the API
        sleep(Duration::from_millis(200)).await;
    }

    let success = rows.iter().any(|r| r.found);
    crate::commands::AnalyzerResult {
        success,
        markdown: String::new(),
        error: if success { String::new() } else { "No categories could be verified via Canopy.".to_string() },
        rows,
    }
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
    let database = app.state::<db::Db>();
    run_competition_canopy(&app, &database, &request).await
}

async fn run_competition_canopy(app: &AppHandle, database: &db::Db, req: &CompetitionCanopyRequest) -> CompetitionResult {
    let client = match CanopyClient::new(&req.canopy_api_key) {
        Ok(c) => c,
        Err(e) => return CompetitionResult { success: false, report: String::new(), error: e },
    };

    let keywords = { let conn = database.0.lock().unwrap(); db::load_mi_search_terms(&conn, &req.folder) };
    if keywords.is_empty() {
        emit_canopy(app, "✗ No search terms found. Run Analyze first.");
        return CompetitionResult { success: false, report: String::new(), error: "No search terms found. Run Analyze first.".to_string() };
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
        let search_results = match client.search(keyword, domain, category_id, 1).await {
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
            let sales = client.get_sales(&sr.asin, domain).await.ok();
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

            sleep(Duration::from_millis(100)).await;
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

    match call_llm(&req.provider, &req.api_key, &req.model, system, &user, 2000).await {
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


// ── Keyword Search via Canopy ────────────────────

use crate::models::{KeywordResult, KeywordSearchResponse};

#[derive(Deserialize)]
pub struct KeywordSearchCanopyRequest {
    pub folder:         String,
    pub seed:           String,
    pub canopy_api_key: String,
}

#[tauri::command]
pub async fn search_keywords_canopy(app: AppHandle, request: KeywordSearchCanopyRequest) -> KeywordSearchResponse {
    let client = match CanopyClient::new(&request.canopy_api_key) {
        Ok(c) => c,
        Err(e) => return KeywordSearchResponse { success: false, results: Vec::new(), error: e },
    };

    emit_canopy(&app, &format!("Keyword search via Canopy: \"{}\"", request.seed));

    // Step 1: Get autocomplete suggestions
    let suggestions = match client.autocomplete(&request.seed, "US", Some("digital-text")).await {
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
        let search_results = match client.search(keyword, "US", Some("digital-text"), 1).await {
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
        let organic: Vec<_> = search_results.iter().filter(|r| !r.is_sponsored).take(3).collect();

        let mut top_daily_sales: Vec<f64> = Vec::new();
        for sr in &organic {
            if sr.asin.is_empty() { continue; }
            if let Ok(sales) = client.get_sales(&sr.asin, "US").await {
                if let Some(daily) = sales.estimated_daily_sales {
                    top_daily_sales.push(daily);
                }
            }
            sleep(Duration::from_millis(100)).await;
        }

        let avg_daily = if top_daily_sales.is_empty() { 0.0 }
            else { top_daily_sales.iter().sum::<f64>() / top_daily_sales.len() as f64 };

        // Volume estimation: avg daily sales of top 3 × multiplier
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
}

// ══════════════════════════════════════════════════════════════════════════════
// NEW FEATURES: Review Mining, Author Analysis, Category Sync, Deep Categories
// ══════════════════════════════════════════════════════════════════════════════

// ── Review Mining ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ReviewMiningRequest {
    pub folder:         String,
    pub canopy_api_key: String,
    pub api_key:        String,
    pub model:          String,
    pub provider:       String,
}

#[derive(Serialize)]
pub struct ReviewMiningResult {
    pub success: bool,
    pub report:  String,
    pub error:   String,
}

/// Pull reviews from top competitor books, feed to AI for reader insight extraction.
#[tauri::command]
pub async fn mine_competitor_reviews(app: AppHandle, request: ReviewMiningRequest) -> ReviewMiningResult {
    let database = app.state::<db::Db>();
    let client = match CanopyClient::new(&request.canopy_api_key) {
        Ok(c) => c,
        Err(e) => return ReviewMiningResult { success: false, report: String::new(), error: e },
    };

    // Get search terms to find comp books
    let keywords = { let conn = database.0.lock().unwrap(); db::load_mi_search_terms(&conn, &request.folder) };
    if keywords.is_empty() {
        emit_canopy(&app, "✗ No search terms found. Run Analyze first.");
        return ReviewMiningResult { success: false, report: String::new(), error: "No search terms found. Run Analyze first.".to_string() };
    }

    emit_canopy(&app, "Mining competitor reviews via Canopy API...");

    // Search top 5 books for the first 2 keywords
    let mut comp_asins: Vec<(String, String)> = Vec::new(); // (asin, title)
    for kw in keywords.iter().take(2) {
        if let Ok(results) = client.search(kw, "US", Some("digital-text"), 1).await {
            for sr in results.iter().filter(|r| !r.is_sponsored).take(3) {
                if !sr.asin.is_empty() && !comp_asins.iter().any(|(a, _)| a == &sr.asin) {
                    comp_asins.push((sr.asin.clone(), sr.title.clone()));
                }
            }
        }
        sleep(Duration::from_millis(200)).await;
    }

    if comp_asins.is_empty() {
        return ReviewMiningResult { success: false, report: String::new(), error: "No competitor books found.".to_string() };
    }

    emit_canopy(&app, &format!("  Found {} comp books. Pulling reviews...", comp_asins.len()));

    // Pull reviews for each comp book
    let mut all_reviews: Vec<(String, Vec<ReviewEntry>)> = Vec::new(); // (title, reviews)
    for (asin, title) in &comp_asins {
        emit_canopy(&app, &format!("  \"{}\"", title));
        // Get positive and negative reviews
        let mut book_reviews = Vec::new();
        match client.get_reviews(asin, "US", 1, Some(5)).await {
            Ok(reviews) => book_reviews.extend(reviews.into_iter().take(5)),
            Err(e) => emit_canopy(&app, &format!("    ⚠ 5-star reviews failed: {}", e)),
        }
        sleep(Duration::from_millis(150)).await;
        match client.get_reviews(asin, "US", 1, Some(1)).await {
            Ok(reviews) => book_reviews.extend(reviews.into_iter().take(3)),
            Err(e) => emit_canopy(&app, &format!("    ⚠ 1-star reviews failed: {}", e)),
        }
        sleep(Duration::from_millis(150)).await;
        match client.get_reviews(asin, "US", 1, Some(3)).await {
            Ok(reviews) => book_reviews.extend(reviews.into_iter().take(3)),
            Err(e) => emit_canopy(&app, &format!("    ⚠ 3-star reviews failed: {}", e)),
        }
        sleep(Duration::from_millis(150)).await;

        if !book_reviews.is_empty() {
            emit_canopy(&app, &format!("    {} reviews collected.", book_reviews.len()));
            all_reviews.push((title.clone(), book_reviews));
        } else {
            emit_canopy(&app, "    ✗ No reviews returned for this book.");
        }
    }

    if all_reviews.is_empty() {
        return ReviewMiningResult { success: false, report: String::new(), error: "Could not pull reviews from any comp book.".to_string() };
    }

    // Build review text for AI
    let mut review_text = String::new();
    for (title, reviews) in &all_reviews {
        review_text.push_str(&format!("\n## \"{}\"\n", title));
        for r in reviews {
            review_text.push_str(&format!("[{}★{}] {}: {}\n",
                r.rating, if r.verified { " ✓" } else { "" }, r.title, r.body));
        }
    }

    emit_canopy(&app, "  Running AI analysis on reviews...");

    let system = r#"You are a publishing strategist. Analyze these competitor book reviews and extract actionable intelligence. Produce a structured report:

1. **What Readers Love** — themes, elements, tropes that get praised repeatedly
2. **What Readers Hate** — common complaints, disappointments, unmet expectations
3. **Reader Language** — exact phrases and words readers use that could go in book descriptions and ad copy
4. **Gap Opportunities** — things readers want but aren't getting from current books
5. **Positioning Advice** — how to position a new book to capture these readers

Be specific. Quote review language directly when useful. Keep it actionable."#;

    let genre_context = {
        let conn = database.0.lock().unwrap();
        db::load_genre_data(&conn, &request.folder).map(|g| g.genre_signals).unwrap_or_default()
    };

    let user = format!("Genre context: {}\n\nCompetitor reviews:\n{}", genre_context, review_text);

    match call_llm(&request.provider, &request.api_key, &request.model, system, &user, 2500).await {
        Ok(report) => {
            let json = serde_json::json!({
                "schema": "review_mining_v1",
                "content_format": "markdown",
                "content": report,
                "books_analyzed": comp_asins.iter().map(|(a, t)| serde_json::json!({"asin": a, "title": t})).collect::<Vec<_>>(),
                "total_reviews": all_reviews.iter().map(|(_, r)| r.len()).sum::<usize>(),
            }).to_string();
            let conn = database.0.lock().unwrap();
            let _ = db::save_document(&conn, &request.folder, "review_mining", &json);
            emit_canopy(&app, "✓ Review mining report saved.");
            ReviewMiningResult { success: true, report: json, error: String::new() }
        }
        Err(e) => ReviewMiningResult { success: false, report: String::new(), error: format!("AI error: {}", e) },
    }
}

// ── Author Catalog Analysis ───────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct AuthorAnalysisRequest {
    pub folder:         String,
    pub canopy_api_key: String,
    pub api_key:        String,
    pub model:          String,
    pub provider:       String,
}

#[derive(Serialize)]
pub struct AuthorAnalysisResult {
    pub success: bool,
    pub report:  String,
    pub error:   String,
}

/// Analyze top competitor authors' catalogs — release cadence, pricing, series strategy.
#[tauri::command]
pub async fn analyze_comp_authors(app: AppHandle, request: AuthorAnalysisRequest) -> AuthorAnalysisResult {
    let database = app.state::<db::Db>();
    let client = match CanopyClient::new(&request.canopy_api_key) {
        Ok(c) => c,
        Err(e) => return AuthorAnalysisResult { success: false, report: String::new(), error: e },
    };

    let keywords = { let conn = database.0.lock().unwrap(); db::load_mi_search_terms(&conn, &request.folder) };
    if keywords.is_empty() {
        emit_canopy(&app, "✗ No search terms found. Run Analyze first.");
        return AuthorAnalysisResult { success: false, report: String::new(), error: "No search terms found. Run Analyze first.".to_string() };
    }

    emit_canopy(&app, "Analyzing competitor authors via Canopy API...");

    // Find unique authors from search results
    let mut author_asins: Vec<(String, String)> = Vec::new(); // (asin, name)
    let mut seen_authors: std::collections::HashSet<String> = std::collections::HashSet::new();

    for kw in keywords.iter().take(3) {
        if let Ok(results) = client.search(kw, "US", Some("digital-text"), 1).await {
            for sr in results.iter().filter(|r| !r.is_sponsored).take(5) {
                if sr.asin.is_empty() { continue; }
                if let Some(author) = &sr.author {
                    if !author.is_empty() && seen_authors.insert(author.to_lowercase()) {
                        // Get the product to find author ASIN if available
                        if let Ok(_product) = client.get_product(&sr.asin, "US").await {
                            author_asins.push((sr.asin.clone(), author.clone()));
                        }
                    }
                }
            }
        }
        sleep(Duration::from_millis(200)).await;
    }

    let authors_to_analyze: Vec<_> = author_asins.into_iter().take(5).collect();
    if authors_to_analyze.is_empty() {
        return AuthorAnalysisResult { success: false, report: String::new(), error: "No competitor authors found.".to_string() };
    }

    emit_canopy(&app, &format!("  {} authors identified. Fetching catalogs...", authors_to_analyze.len()));

    // For each author, search for more of their books
    let mut author_data: Vec<serde_json::Value> = Vec::new();
    for (_book_asin, author_name) in &authors_to_analyze {
        emit_canopy(&app, &format!("  → {}", author_name));

        // Search for more books by this author
        let search_term = format!("{}", author_name);
        let books = match client.search(&search_term, "US", Some("digital-text"), 1).await {
            Ok(results) => results.iter()
                .filter(|r| r.author.as_deref() == Some(author_name.as_str()))
                .take(10)
                .map(|r| serde_json::json!({
                    "title": r.title,
                    "price": r.price,
                    "rating": r.rating,
                    "reviews": r.review_count,
                }))
                .collect::<Vec<_>>(),
            Err(_) => Vec::new(),
        };

        let book_count = books.len();
        author_data.push(serde_json::json!({
            "name": author_name,
            "book_count": book_count,
            "books": books,
        }));
        emit_canopy(&app, &format!("    {} books found.", book_count));
        sleep(Duration::from_millis(200)).await;
    }

    emit_canopy(&app, "  Running AI analysis on author catalogs...");

    let system = r#"You are a publishing strategist. Analyze these competitor author catalogs and extract strategic intelligence. Produce a structured report:

1. **Catalog Size & Release Cadence** — how many books, how often they publish
2. **Pricing Strategy** — price points across their catalog, any patterns (loss leaders, premium pricing)
3. **Series vs Standalone** — do they write series? How long? Does the first book price differently?
4. **Review Performance** — average ratings, which books perform best
5. **Strategic Takeaways** — what can a debut author learn from their approach?

Be specific and data-driven."#;

    let genre_context = {
        let conn = database.0.lock().unwrap();
        db::load_genre_data(&conn, &request.folder).map(|g| g.genre_signals).unwrap_or_default()
    };

    let author_summary = serde_json::to_string_pretty(&author_data).unwrap_or_default();
    let user = format!("Genre context: {}\n\nCompetitor author catalogs:\n{}", genre_context, author_summary);

    match call_llm(&request.provider, &request.api_key, &request.model, system, &user, 2500).await {
        Ok(report) => {
            let json = serde_json::json!({
                "schema": "author_analysis_v1",
                "content_format": "markdown",
                "content": report,
                "authors_analyzed": author_data,
            }).to_string();
            let conn = database.0.lock().unwrap();
            let _ = db::save_document(&conn, &request.folder, "author_analysis", &json);
            emit_canopy(&app, "✓ Author catalog analysis saved.");
            AuthorAnalysisResult { success: true, report: json, error: String::new() }
        }
        Err(e) => AuthorAnalysisResult { success: false, report: String::new(), error: format!("AI error: {}", e) },
    }
}

// ── Deep Category Analysis ────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
pub struct DeepCategoryBook {
    pub rank:         u32,
    pub asin:         String,
    pub title:        String,
    pub author:       Option<String>,
    pub price:        Option<f64>,
    pub rating:       Option<f64>,
    pub review_count: Option<u32>,
    pub daily_sales:  Option<f64>,
    pub ku:           bool,
    pub indie:        bool,
}

#[derive(Serialize)]
pub struct DeepCategoryResult {
    pub success:    bool,
    pub category:   String,
    pub books:      Vec<DeepCategoryBook>,
    pub stats:      DeepCategoryStats,
    pub error:      String,
}

#[derive(Serialize)]
pub struct DeepCategoryStats {
    pub total_books:   usize,
    pub avg_price:     String,
    pub avg_rating:    String,
    pub avg_reviews:   String,
    pub indie_pct:     String,
    pub ku_pct:        String,
    pub sales_rank_1:  String,
    pub sales_rank_10: String,
    pub sales_rank_25: String,
    pub sales_rank_50: String,
}

/// Deep category penetration — pull ranks 1-50 with full product details.
#[tauri::command]
pub async fn deep_category_analysis(app: AppHandle, canopy_api_key: String, category_path: String, node_id: String, store: String) -> DeepCategoryResult {
    let client = match CanopyClient::new(&canopy_api_key) {
        Ok(c) => c,
        Err(e) => return DeepCategoryResult { success: false, category: category_path, books: Vec::new(), stats: empty_deep_stats(), error: e },
    };

    let domain = store_to_domain(&store);
    emit_canopy(&app, &format!("Deep analysis: {} (node {})", category_path, node_id));

    // Pull page 1 (typically 20-30 results) + page 2 to get ~50 books
    let mut all_books: Vec<BestsellerEntry> = Vec::new();
    for page in 1..=2 {
        match client.get_category_products(&node_id, domain, page).await {
            Ok(entries) => {
                emit_canopy(&app, &format!("  Page {}: {} books", page, entries.len()));
                all_books.extend(entries);
            }
            Err(e) => {
                if page == 1 {
                    return DeepCategoryResult { success: false, category: category_path, books: Vec::new(), stats: empty_deep_stats(), error: e };
                }
            }
        }
        sleep(Duration::from_millis(200)).await;
    }

    emit_canopy(&app, &format!("  {} total books. Fetching details...", all_books.len()));

    // Get detailed info for each book
    let mut deep_books: Vec<DeepCategoryBook> = Vec::new();
    for (i, entry) in all_books.iter().take(50).enumerate() {
        if entry.asin.is_empty() { continue; }

        let sales = client.get_sales(&entry.asin, domain).await.ok();
        let product = client.get_product(&entry.asin, domain).await.ok();

        let daily = sales.as_ref().and_then(|s| s.estimated_daily_sales);
        let ku = product.as_ref().map(|p| p.kindle_unlimited).unwrap_or(false);
        let indie = product.as_ref().and_then(|p| p.publisher.as_ref()).map(|pub_name| {
            let lower = pub_name.to_lowercase();
            lower.contains("independently published") || lower.contains("self-published") || lower.contains("kindle direct")
        }).unwrap_or(false);

        deep_books.push(DeepCategoryBook {
            rank: entry.rank,
            asin: entry.asin.clone(),
            title: entry.title.clone(),
            author: entry.author.clone(),
            price: entry.price,
            rating: entry.rating,
            review_count: entry.review_count,
            daily_sales: daily,
            ku, indie,
        });

        if (i + 1) % 10 == 0 {
            emit_canopy(&app, &format!("  {}/{} books detailed.", i + 1, all_books.len().min(50)));
        }
        sleep(Duration::from_millis(100)).await;
    }

    // Compute stats
    let total = deep_books.len();
    let prices: Vec<f64> = deep_books.iter().filter_map(|b| b.price).collect();
    let ratings: Vec<f64> = deep_books.iter().filter_map(|b| b.rating).collect();
    let reviews: Vec<f64> = deep_books.iter().filter_map(|b| b.review_count.map(|r| r as f64)).collect();
    let indie_count = deep_books.iter().filter(|b| b.indie).count();
    let ku_count = deep_books.iter().filter(|b| b.ku).count();

    let avg = |v: &[f64]| if v.is_empty() { "N/A".to_string() } else { format!("{:.1}", v.iter().sum::<f64>() / v.len() as f64) };
    let sales_at = |rank: usize| deep_books.get(rank.saturating_sub(1))
        .and_then(|b| b.daily_sales)
        .map(|d| format!("{:.0}/day", d))
        .unwrap_or_else(|| "N/A".to_string());

    let stats = DeepCategoryStats {
        total_books: total,
        avg_price: if prices.is_empty() { "N/A".to_string() } else { format!("${:.2}", prices.iter().sum::<f64>() / prices.len() as f64) },
        avg_rating: avg(&ratings),
        avg_reviews: avg(&reviews),
        indie_pct: if total > 0 { format!("{:.0}%", 100.0 * indie_count as f64 / total as f64) } else { "N/A".to_string() },
        ku_pct: if total > 0 { format!("{:.0}%", 100.0 * ku_count as f64 / total as f64) } else { "N/A".to_string() },
        sales_rank_1: sales_at(1),
        sales_rank_10: sales_at(10),
        sales_rank_25: sales_at(25),
        sales_rank_50: sales_at(50),
    };

    emit_canopy(&app, &format!("✓ Deep analysis complete: {} books, avg ${}, {}% indie, {}% KU",
        total, stats.avg_price, stats.indie_pct, stats.ku_pct));

    DeepCategoryResult { success: true, category: category_path, books: deep_books, stats, error: String::new() }
}

fn empty_deep_stats() -> DeepCategoryStats {
    DeepCategoryStats {
        total_books: 0, avg_price: "N/A".to_string(), avg_rating: "N/A".to_string(),
        avg_reviews: "N/A".to_string(), indie_pct: "N/A".to_string(), ku_pct: "N/A".to_string(),
        sales_rank_1: "N/A".to_string(), sales_rank_10: "N/A".to_string(),
        sales_rank_25: "N/A".to_string(), sales_rank_50: "N/A".to_string(),
    }
}
