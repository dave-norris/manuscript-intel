// genre_analyzer.rs — Full manuscript analysis pipeline
//
// Three commands:
//   generate_summaries  — Phase 1 only: chapter by chapter, saves JSON per chapter
//   analyze_genre       — Phase 2 only: reads summaries, produces genre-analysis.md
//                         + genre-data.json; auto-generates summaries if missing
//   run_full_analysis   — Phase 1 + 2 + 3: everything, including PR scrape

use std::path::{Path, PathBuf};
use std::fs;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tauri_plugin_dialog::DialogExt;

use crate::commands::call_anthropic;
use crate::models;
use crate::pr_scraper;

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ChapterSummary {
    pub file:       String,
    pub title:      String,
    pub signals:    String,
    pub word_count: usize,
}

/// Structured data written to genre-data.json — used by Phase 3 to drive PR.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GenreData {
    pub generated: String,
    pub industry: IndustryGenre,
    pub kdp: KdpCategories,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct IndustryGenre {
    pub ebook: String,
    pub print: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct KdpCategories {
    pub ebook: Vec<String>,
    pub print: Vec<String>,
}

#[derive(Serialize)]
pub struct GenreResult {
    pub success: bool,
    pub report:  String,
    pub error:   String,
}

#[derive(Deserialize)]
pub struct FolderRequest {
    pub folder:  String,
    pub api_key: String,
    pub model:   String,
}

// ── Commands ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn pick_manuscript_folder(app: AppHandle) -> Result<String, String> {
    use tauri_plugin_dialog::FilePath;
    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog()
        .file()
        .set_title("Select Manuscript Folder")
        .pick_folder(move |result| { let _ = tx.send(result); });
    match rx.recv() {
        Ok(Some(FilePath::Path(p))) => Ok(p.to_string_lossy().to_string()),
        Ok(_) => Err("No folder selected".to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// Phase 1 only — generate chapter summaries.
#[tauri::command]
pub async fn generate_summaries(app: AppHandle, request: FolderRequest) -> GenreResult {
    tokio::task::spawn_blocking(move || {
        let folder = PathBuf::from(&request.folder);
        if !folder.exists() { return err("Folder does not exist."); }

        let summaries_dir = folder.join("_analysis").join("summaries");
        if let Err(e) = fs::create_dir_all(&summaries_dir) {
            return err(&format!("Cannot create _analysis/summaries: {}", e));
        }

        crate::reset_cancel();
        let chapters = collect_chapters(&folder);
        if chapters.is_empty() { return err("No .md files found."); }

        emit(&app, &format!("Found {} chapter file(s). Starting summaries...", chapters.len()));

        let (done, skipped) = phase1_summaries(
            &app, &chapters, &summaries_dir, &request.api_key
        );

        GenreResult {
            success: true,
            report: format!("✓ {} summarized, {} already done.", done, skipped),
            error: String::new(),
        }
    }).await.unwrap()
}

/// Phase 2 only — read summaries → genre-analysis.md + genre-data.json.
/// Auto-runs Phase 1 if summaries are missing.
#[tauri::command]
pub async fn analyze_genre(app: AppHandle, request: FolderRequest) -> GenreResult {
    tokio::task::spawn_blocking(move || {
        let folder = PathBuf::from(&request.folder);
        if !folder.exists() { return err("Folder does not exist."); }

        let analysis_dir  = folder.join("_analysis");
        let summaries_dir = analysis_dir.join("summaries");
        if let Err(e) = fs::create_dir_all(&summaries_dir) {
            return err(&format!("Cannot create _analysis/summaries: {}", e));
        }

        // Load existing summaries
        let mut summaries = load_summaries(&summaries_dir);

        // If none found, auto-run Phase 1 first
        if summaries.is_empty() {
            emit(&app, "No summaries found — running Phase 1 first...");
            let chapters = collect_chapters(&folder);
            if chapters.is_empty() { return err("No .md files found."); }
            phase1_summaries(&app, &chapters, &summaries_dir, &request.api_key);
            summaries = load_summaries(&summaries_dir);
        }

        if summaries.is_empty() { return err("Could not produce any chapter summaries."); }

        emit(&app, &format!("Phase 2: Analyzing {} chapter summaries...", summaries.len()));
        phase2_analyze(&app, &analysis_dir, &summaries, &request.api_key, &request.model)
    }).await.unwrap()
}

/// Full pipeline: Phase 1 + 2 + 3 (PR scrape).
#[tauri::command]
pub async fn run_full_analysis(app: AppHandle, request: FolderRequest) -> GenreResult {
    tokio::task::spawn_blocking(move || {
        let folder = PathBuf::from(&request.folder);
        if !folder.exists() { return err("Folder does not exist."); }

        let analysis_dir  = folder.join("_analysis");
        let summaries_dir = analysis_dir.join("summaries");
        if let Err(e) = fs::create_dir_all(&summaries_dir) {
            return err(&format!("Cannot create _analysis/summaries: {}", e));
        }

        // ── Phase 1 ──────────────────────────────────────────────────────────
        let mut summaries = load_summaries(&summaries_dir);
        if summaries.is_empty() {
            emit(&app, "Phase 1: Generating chapter summaries...");
            let chapters = collect_chapters(&folder);
            if chapters.is_empty() { return err("No .md files found."); }
            phase1_summaries(&app, &chapters, &summaries_dir, &request.api_key);
            summaries = load_summaries(&summaries_dir);
        } else {
            emit(&app, &format!("Phase 1: {} summaries already exist — skipping.", summaries.len()));
        }

        if summaries.is_empty() { return err("No chapter summaries available."); }

        // ── Phase 2 ──────────────────────────────────────────────────────────
        // Check if genre-data.json already exists and is fresh enough
        let data_path = analysis_dir.join("genre-data.json");
        let genre_data = if data_path.exists() {
            emit(&app, "Phase 2: genre-data.json exists — loading...");
            match fs::read_to_string(&data_path)
                .ok()
                .and_then(|s| serde_json::from_str::<GenreData>(&s).ok())
            {
                Some(d) => d,
                None => {
                    emit(&app, "  Could not parse genre-data.json — re-running Phase 2...");
                    match phase2_analyze(&app, &analysis_dir, &summaries, &request.api_key, &request.model) {
                        r if r.success => match load_genre_data(&data_path) {
                            Some(d) => d,
                            None => return err("Phase 2 produced no genre-data.json"),
                        },
                        r => return r,
                    }
                }
            }
        } else {
            emit(&app, "Phase 2: Running genre analysis...");
            match phase2_analyze(&app, &analysis_dir, &summaries, &request.api_key, &request.model) {
                r if r.success => match load_genre_data(&data_path) {
                    Some(d) => d,
                    None => return err("Phase 2 produced no genre-data.json"),
                },
                r => return r,
            }
        };

        emit(&app, &format!(
            "  KDP ebook paths: {}", genre_data.kdp.ebook.join(", ")
        ));
        emit(&app, &format!(
            "  KDP print paths: {}", genre_data.kdp.print.join(", ")
        ));

        // ── Phase 3: PR scrape ────────────────────────────────────────────────
        emit(&app, "Phase 3: Scraping Publisher Rocket...");

        let ebook_results = pr_scraper::scrape_category_paths(
            &app, &genre_data.kdp.ebook, "Kindle", "Selectable Excluding Ghosts"
        );
        let print_results = pr_scraper::scrape_category_paths(
            &app, &genre_data.kdp.print, "Books", "Selectable Excluding Ghosts"
        );

        // ── Build full report ─────────────────────────────────────────────────
        emit(&app, "Building full report...");
        let now = chrono::Utc::now().format("%B %-d, %Y %H:%M UTC").to_string();

        // Load the human genre report for context
        let genre_report = fs::read_to_string(analysis_dir.join("genre-analysis.md"))
            .unwrap_or_default();

        let mut lines = vec![
            "# Full Genre & Market Analysis".to_string(),
            format!("Generated: {}", now),
            String::new(),
            "---".to_string(),
            String::new(),
        ];

        // Embed the genre analysis report
        lines.push(genre_report.clone());
        lines.push(String::new());
        lines.push("---".to_string());
        lines.push(String::new());

        // Ebook category results
        lines.push("## KDP Kindle Ebook — Market Data".to_string());
        lines.push(String::new());
        if ebook_results.is_empty() {
            lines.push("*No ebook category data scraped.*".to_string());
        } else {
            for r in &ebook_results {
                lines.push(format!("### {}", r.path));
                lines.push(String::new());
                if !r.stats.is_empty() {
                    lines.push(format!("- **Sales to reach #1:** {}", r.stats.sales_to_one));
                    lines.push(format!("- **Sales to reach #10:** {}", r.stats.sales_to_ten));
                    lines.push(format!("- **Publisher books:** {}", r.stats.publisher_pct));
                    lines.push(format!("- **KU books:** {}", r.stats.ku_pct));
                    lines.push(String::new());
                }
                if !r.keywords.is_empty() {
                    lines.push("**Keywords**".to_string());
                    lines.push(String::new());
                    lines.push(r.keywords.clone());
                    lines.push(String::new());
                }
                lines.push("---".to_string());
                lines.push(String::new());
            }
        }

        // Print category results
        lines.push("## KDP Print Books — Market Data".to_string());
        lines.push(String::new());
        if print_results.is_empty() {
            lines.push("*No print category data scraped.*".to_string());
        } else {
            for r in &print_results {
                lines.push(format!("### {}", r.path));
                lines.push(String::new());
                if !r.stats.is_empty() {
                    lines.push(format!("- **Sales to reach #1:** {}", r.stats.sales_to_one));
                    lines.push(format!("- **Sales to reach #10:** {}", r.stats.sales_to_ten));
                    lines.push(format!("- **Publisher books:** {}", r.stats.publisher_pct));
                    lines.push(format!("- **KU books:** {}", r.stats.ku_pct));
                    lines.push(String::new());
                }
                if !r.keywords.is_empty() {
                    lines.push("**Keywords**".to_string());
                    lines.push(String::new());
                    lines.push(r.keywords.clone());
                    lines.push(String::new());
                }
                lines.push("---".to_string());
                lines.push(String::new());
            }
        }

        let full_report = lines.join("\n");

        // Save full report
        let report_path = analysis_dir.join("full-report.md");
        let _ = fs::write(&report_path, &full_report);
        emit(&app, &format!("✓ Full report saved to {}", report_path.display()));

        GenreResult { success: true, report: full_report, error: String::new() }
    }).await.unwrap()
}



// ── Analysis state check ──────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct AnalysisState {
    pub has_folder:      bool,
    pub summary_count:   usize,  // number of chapter summary JSONs
    pub has_genre_data:  bool,   // genre-data.json exists
    pub has_full_report: bool,   // full-report.md exists
    pub has_keywords:    bool,   // kdp-keywords.md exists
    pub has_pr_keywords: bool,   // pr-keywords.json exists
    pub has_competition: bool,   // competition-report.md exists
}

#[tauri::command]
pub async fn check_analysis_state(folder: String) -> AnalysisState {
    tokio::task::spawn_blocking(move || {
        let folder_path  = std::path::PathBuf::from(&folder);
        let analysis_dir = folder_path.join("_analysis");
        let summaries_dir = analysis_dir.join("summaries");

        let has_folder = folder_path.exists();

        let summary_count = if summaries_dir.exists() {
            std::fs::read_dir(&summaries_dir)
                .map(|d| d.flatten()
                    .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
                    .count())
                .unwrap_or(0)
        } else {
            0
        };

        AnalysisState {
            has_folder,
            summary_count,
            has_genre_data:  analysis_dir.join("genre-data.json").exists(),
            has_full_report: analysis_dir.join("full-report.md").exists(),
            has_keywords:    analysis_dir.join("kdp-keywords.md").exists(),
            has_pr_keywords: analysis_dir.join("pr-keywords.json").exists(),
            has_competition: analysis_dir.join("competition-report.md").exists(),
        }
    }).await.unwrap()
}


// ── PR Keyword Generator ──────────────────────────────────────────────────────
// Generates short 2-4 word phrases suitable for PR Competition Analyzer.
// Completely separate from KDP keyword strings.

#[tauri::command]
pub async fn generate_pr_keywords(app: AppHandle, request: KeywordRequest) -> GenreResult {
    tokio::task::spawn_blocking(move || {
        let folder = PathBuf::from(&request.folder);
        let analysis_dir = folder.join("_analysis");

        if !analysis_dir.exists() {
            return err("No _analysis folder. Run Analyze first.");
        }

        let genre_data = match load_genre_data(&analysis_dir.join("genre-data.json")) {
            Some(d) => d,
            None => return err("genre-data.json not found. Run Analyze first."),
        };

        emit(&app, "Generating PR Competition Analyzer search terms...");
        emit(&app, &format!("  Genre: {}", genre_data.industry.ebook));

        let system = r#"You are a Publisher Rocket expert. Generate short search phrases for the Competition Analyzer tool.

Publisher Rocket Competition Analyzer works like Amazon search — it needs SHORT, SPECIFIC phrases that real readers type.

Rules:
- 2-4 words maximum per phrase
- Plain English, no special characters
- Think like a reader browsing Amazon, not a marketer
- Phrases should find competing books in the same genre niche
- Include: genre combinations, setting descriptors, theme words, reader mood phrases

Return ONLY a JSON array of strings. No markdown, no preamble. Example:
["christian historical fiction", "first century rome", "biblical mystery", "faith romance clean"]"#;

        let user = format!(
            "Book genre: {}
KDP categories: {}
Genre signals from chapters: {}",
            genre_data.industry.ebook,
            genre_data.kdp.ebook.iter()
                .map(|p| p.split('>').last().unwrap_or(p).trim().to_string())
                .collect::<Vec<_>>().join(", "),
            // Load a snippet of genre signals for context
            fs::read_to_string(analysis_dir.join("genre-analysis.md"))
                .map(|s| s[..s.len().min(500)].to_string())
                .unwrap_or_default()
        );

        match call_anthropic(&request.api_key, models::HAIKU, system, &user, 300) {
            Err(e) => err(&format!("AI error: {}", e)),
            Ok(raw) => {
                let clean = raw.trim()
                    .trim_start_matches("```json").trim_start_matches("```")
                    .trim_end_matches("```").trim();

                match serde_json::from_str::<Vec<String>>(clean) {
                    Err(e) => err(&format!("Parse error: {} | got: {}", e, &clean[..clean.len().min(200)])),
                    Ok(keywords) => {
                        // Save to pr-keywords.json
                        let out_path = analysis_dir.join("pr-keywords.json");
                        if let Ok(json) = serde_json::to_string_pretty(&keywords) {
                            let _ = fs::write(&out_path, &json);
                        }

                        emit(&app, &format!("  ✓ Generated {} PR search terms:", keywords.len()));
                        for kw in &keywords {
                            emit(&app, &format!("    • {}", kw));
                        }

                        // Build a simple display report
                        let now = chrono::Utc::now().format("%B %-d, %Y %H:%M UTC").to_string();
                        let mut md = vec![
                            "# PR Competition Analyzer Keywords".to_string(),
                            format!("Generated: {}", now),
                            String::new(),
                            "> These short phrases are for use with Publisher Rocket's Competition Analyzer.".to_string(),
                            "> They are NOT the same as your KDP keyword strings.".to_string(),
                            String::new(),
                        ];
                        for kw in &keywords {
                            md.push(format!("- `{}`", kw));
                        }
                        md.push(String::new());

                        GenreResult {
                            success: true,
                            report: md.join("\n"),
                            error: String::new(),
                        }
                    }
                }
            }
        }
    }).await.unwrap()
}

// ── Keyword Optimizer ─────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct KeywordRequest {
    pub folder:  String,
    pub api_key: String,
    pub model:   String,
}

/// Read full-report.md + genre-data.json from _analysis/, extract all keywords,
/// and ask Sonnet to produce 7 optimized KDP keyword strings.
#[tauri::command]
pub async fn optimize_keywords(app: AppHandle, request: KeywordRequest) -> GenreResult {
    tokio::task::spawn_blocking(move || {
        let folder = PathBuf::from(&request.folder);
        let analysis_dir = folder.join("_analysis");

        if !analysis_dir.exists() {
            return err("No _analysis folder found. Run Full Analysis first.");
        }

        // Load genre context
        let genre_data = match load_genre_data(&analysis_dir.join("genre-data.json")) {
            Some(d) => d,
            None => return err("genre-data.json not found. Run Full Analysis first."),
        };

        // Load full report for keyword extraction
        let full_report = match fs::read_to_string(analysis_dir.join("full-report.md")) {
            Ok(s) => s,
            Err(_) => match fs::read_to_string(analysis_dir.join("genre-analysis.md")) {
                Ok(s) => s,
                Err(_) => return err("No report found. Run Analyze or Full Analysis first."),
            },
        };

        emit(&app, "Extracting keywords from report...");

        // Extract keyword sections from the report (present after Full Analysis with PR scrape)
        let keywords_text = extract_keyword_sections(&full_report);
        emit(&app, &format!("  Extracted {} chars of Publisher Rocket keyword material.", keywords_text.len()));

        // If no PR keywords found, work from genre signals instead.
        // This happens when Analyze was run but not Full Analysis.
        let (keywords_context, source_note) = if keywords_text.is_empty() {
            emit(&app, "  No PR keyword sections found — using genre signals and category paths.");
            let genre_signals = fs::read_to_string(analysis_dir.join("genre-analysis.md"))
                .unwrap_or_default();
            (
                genre_signals,
                "*(Note: Generated from genre analysis only. Run Full Analysis to include Publisher Rocket keyword data for better results.)*"
            )
        } else {
            (keywords_text, "*(Generated from Publisher Rocket keyword data.)*")
        };

        let analysis_model = models::resolve_analysis_model(&request.model);
        emit(&app, &format!("Asking {} to optimize keywords...", analysis_model));

        match call_keyword_optimizer(
            &request.api_key,
            analysis_model,
            &genre_data,
            &keywords_context,
        ) {
            Err(e) => err(&format!("AI error: {}", e)),
            Ok(result_md) => {
                // Prepend source note
                let final_md = format!("{}

{}", source_note, result_md);
                let out_path = analysis_dir.join("kdp-keywords.md");
                let _ = fs::write(&out_path, &final_md);
                emit(&app, &format!("✓ Saved to {}", out_path.display()));
                GenreResult { success: true, report: final_md, error: String::new() }
            }
        }
    }).await.unwrap()
}

fn extract_keyword_sections(report: &str) -> String {
    // Pull out lines between "**Keywords**" markers and the next "---" separator
    let mut out = Vec::new();
    let mut capturing = false;
    for line in report.lines() {
        let trimmed = line.trim();
        if trimmed == "**Keywords**" || trimmed == "### Keywords" {
            capturing = true;
            continue;
        }
        if capturing {
            if trimmed == "---" || trimmed.starts_with("### ") {
                capturing = false;
                out.push(String::new()); // blank line between sections
            } else if !trimmed.is_empty() {
                out.push(line.to_string());
            }
        }
    }
    out.join("\n")
}

fn call_keyword_optimizer(
    api_key: &str,
    model: &str,
    genre_data: &GenreData,
    keywords_text: &str,
) -> Result<String, String> {
    let system = r#"You are an Amazon KDP keyword strategist helping an indie author maximize book discoverability.

You will receive:
1. The book's genre and KDP categories
2. Raw keywords scraped from Publisher Rocket's Category Search

Your job is to produce exactly 7 KDP keyword strings ready to paste into the KDP keyword fields.

Rules:
- Each string must be 50 characters or fewer (count carefully — this is a hard limit)
- Each string should be a natural search phrase a reader would actually type on Amazon
- Use multi-word phrases, not single words — Amazon already indexes your title and categories
- Do NOT repeat words that are already in the book's categories
- Prioritize phrases that combine genre + reader intent (e.g. "clean christian romance suspense")
- Vary the strings — cover different angles: setting, theme, reader mood, comp authors, tropes
- No punctuation except spaces and hyphens
- All lowercase

Return a JSON object with this exact structure — nothing else:
{
  "keywords": [
    { "string": "the keyword phrase here", "chars": 23, "rationale": "one sentence why" },
    ... (exactly 7 items)
  ],
  "strategy": "One paragraph explaining the overall keyword strategy."
}"#;

    let user = format!(
        "Genre (ebook): {}
Genre (print): {}
KDP ebook categories: {}
KDP print categories: {}

Raw keywords from Publisher Rocket:

{}",
        genre_data.industry.ebook,
        genre_data.industry.print,
        genre_data.kdp.ebook.join(", "),
        genre_data.kdp.print.join(", "),
        keywords_text
    );

    let raw = call_anthropic(api_key, model, system, &user, 1000)?;
    let clean = raw.trim()
        .trim_start_matches("```json").trim_start_matches("```")
        .trim_end_matches("```").trim();

    let v: serde_json::Value = serde_json::from_str(clean)
        .map_err(|e| format!("JSON parse error: {} | got: {}", e, &clean[..clean.len().min(400)]))?;

    let keywords = v["keywords"].as_array()
        .ok_or("Missing 'keywords' array in response")?;
    let strategy = v["strategy"].as_str().unwrap_or("").to_string();

    // Build markdown report
    let now = chrono::Utc::now().format("%B %-d, %Y %H:%M UTC").to_string();
    let mut md = vec![
        "# KDP Keyword Strings".to_string(),
        format!("Generated: {}", now),
        String::new(),
        "> These 7 strings are ready to paste directly into KDP's keyword fields.".to_string(),
        "> Each is verified to be 50 characters or fewer.".to_string(),
        String::new(),
        "---".to_string(),
        String::new(),
        "## Your 7 KDP Keyword Strings".to_string(),
        String::new(),
    ];

    for (i, kw) in keywords.iter().enumerate() {
        let s    = kw["string"].as_str().unwrap_or("");
        let chars = kw["chars"].as_u64().unwrap_or(s.len() as u64);
        let why  = kw["rationale"].as_str().unwrap_or("");
        // Double-check character count
        let actual = s.len();
        let flag = if actual > 50 { " ⚠️ OVER 50 CHARS — shorten before using" } else { "" };
        md.push(format!("**{}. `{}`**{}", i + 1, s, flag));
        md.push(format!("*{} characters — {}*", chars.max(actual as u64), why));
        md.push(String::new());
    }

    md.push("---".to_string());
    md.push(String::new());
    md.push("## Strategy".to_string());
    md.push(String::new());
    md.push(strategy);
    md.push(String::new());
    md.push("---".to_string());
    md.push(String::new());
    md.push("## How to Use".to_string());
    md.push(String::new());
    md.push("1. Go to KDP → Your Books → Edit eBook Details".to_string());
    md.push("2. Scroll to **Keywords** (7 fields)".to_string());
    md.push("3. Paste one string per field".to_string());
    md.push("4. Do NOT use commas inside a field — KDP treats the whole field as one phrase".to_string());
    md.push("5. Do NOT repeat words already in your title, subtitle, or categories".to_string());
    md.push(String::new());

    Ok(md.join("\n"))
}

// ── Phase implementations ─────────────────────────────────────────────────────

/// Run Phase 1 — returns (newly_summarized, skipped).
fn phase1_summaries(
    app: &AppHandle,
    chapters: &[PathBuf],
    summaries_dir: &Path,
    api_key: &str,
) -> (usize, usize) {
    let mut done = 0usize;
    let mut skipped = 0usize;

    for (i, chapter_path) in chapters.iter().enumerate() {
        let fname = chapter_path.file_name()
            .unwrap_or_default().to_string_lossy().to_string();
        let summary_path = summaries_dir.join(
            format!("{:03}-{}.json", i + 1, sanitize(&fname))
        );

        if summary_path.exists() {
            emit(app, &format!("  [{}/{}] SKIP: {}", i + 1, chapters.len(), fname));
            skipped += 1;
            continue;
        }

        emit(app, &format!("  [{}/{}] Summarizing: {}", i + 1, chapters.len(), fname));

        let content = match fs::read_to_string(chapter_path) {
            Ok(c) if !c.trim().is_empty() => c,
            Ok(_)  => { emit(app, "    ⚠ Empty file — skipping."); continue; }
            Err(e) => { emit(app, &format!("    ⚠ Read error: {}", e)); continue; }
        };

        let word_count = content.split_whitespace().count();
        emit(app, &format!("    {} words", word_count));

        let truncated = truncate_words(&content, 8000);

        match summarize_chapter(api_key, models::HAIKU, &fname, &truncated) {
            Ok(signals) => {
                let summary = ChapterSummary {
                    file: fname.clone(),
                    title: extract_title(&content).unwrap_or_else(|| fname.clone()),
                    signals,
                    word_count,
                };
                if let Ok(json) = serde_json::to_string_pretty(&summary) {
                    let _ = fs::write(&summary_path, json);
                }
                emit(app, &format!("    ✓ Done ({} signal chars)", summary.signals.len()));
                done += 1;
            }
            Err(e) => emit(app, &format!("    ⚠ AI error: {}", e)),
        }

        // Check for cancellation after each chapter
        if crate::is_cancelled() {
            emit(app, "⚠ Cancelled by user.");
            break;
        }
    }

    emit(app, &format!("Phase 1 complete — {} new, {} skipped.", done, skipped));
    (done, skipped)
}

/// Run Phase 2 — genre analysis → genre-analysis.md + genre-data.json.
fn phase2_analyze(
    app: &AppHandle,
    analysis_dir: &Path,
    summaries: &[ChapterSummary],
    api_key: &str,
    model: &str,
) -> GenreResult {
    let analysis_model = models::resolve_analysis_model(model);
    let combined = build_combined_context(summaries);

    emit(app, &format!(
        "  Sending {} chapter summaries ({} chars) to {}...",
        summaries.len(), combined.len(), analysis_model
    ));

    match call_ai_genre_analysis(api_key, analysis_model, &combined) {
        Err(e) => err(&format!("Phase 2 AI error: {}", e)),
        Ok((report_md, genre_data)) => {
            // Save human report
            let report_path = analysis_dir.join("genre-analysis.md");
            let _ = fs::write(&report_path, &report_md);
            emit(app, &format!("  ✓ genre-analysis.md saved"));

            // Save machine data
            let data_path = analysis_dir.join("genre-data.json");
            if let Ok(json) = serde_json::to_string_pretty(&genre_data) {
                let _ = fs::write(&data_path, json);
                emit(app, &format!("  ✓ genre-data.json saved"));
            }

            GenreResult { success: true, report: report_md, error: String::new() }
        }
    }
}

// ── AI calls ──────────────────────────────────────────────────────────────────

fn summarize_chapter(api_key: &str, model: &str, filename: &str, content: &str)
    -> Result<String, String>
{
    let system = r#"You are a literary analyst specializing in book genre classification for publishing.

Extract ONLY the genre signals from this chapter — not a plot summary.

Cover:
- Setting: time period, location, world type (historical, contemporary, fantasy, etc.)
- Tone: dark/cozy/literary/commercial/inspirational/gritty/romantic
- Themes: faith, redemption, justice, love, survival, identity, etc.
- Conflict type: internal/external, romantic, mystery, adventure, spiritual
- Romantic elements: present/absent, heat level (clean/sweet/sensual/explicit)
- Faith/spiritual elements: Christian, general spiritual, secular
- Supernatural: none/mild/heavy
- Pacing: fast/slow, action/character/dialogue driven
- Narrative voice: POV, distance, register
- Recognizable tropes or genre conventions

Write 2-3 dense paragraphs. Be specific."#;

    call_anthropic(api_key, model, system,
        &format!("Chapter: {}\n\n---\n\n{}", filename, content), 600)
}

/// Returns (human_markdown_report, structured_genre_data).
fn call_ai_genre_analysis(api_key: &str, model: &str, combined: &str)
    -> Result<(String, GenreData), String>
{
    let system = r#"You are a senior publishing consultant specializing in Amazon KDP and the broader ebook/print marketplace.

Analyze the provided chapter genre-signal summaries and return a JSON object with this EXACT structure:

{
  "industry_ebook": "Primary genre / subgenre for ebook market",
  "industry_print": "Primary genre / subgenre for print market",
  "kdp_ebook": [
    "Full > Category > Path > Here",
    "Second > Full > Path"
  ],
  "kdp_print": [
    "Full > Category > Path > Here",
    "Second > Full > Path"
  ],
  "genre_signals": "One paragraph summary of dominant genre signals.",
  "comps_ebook": ["Title by Author (Year)", "Title by Author (Year)"],
  "comps_print": ["Title by Author (Year)", "Title by Author (Year)"],
  "reader_demographic": "Description of the target reader",
  "bookstore_shelving": "Where this would be shelved in a physical bookstore",
  "marketing_notes": ["Note 1", "Note 2", "Note 3"]
}

Rules:
- KDP paths must be real, full paths from the Kindle Store category tree
- Include exactly 2 category paths per format (ebook and print)
- Return ONLY the JSON object, no markdown fences, no preamble"#;

    let raw = call_anthropic(api_key, model, system,
        &format!("Genre signals from all chapters:\n\n{}", combined), 1500)?;

    let clean = raw.trim()
        .trim_start_matches("```json").trim_start_matches("```")
        .trim_end_matches("```").trim();

    let v: serde_json::Value = serde_json::from_str(clean)
        .map_err(|e| format!("JSON parse error: {} | got: {}", e, &clean[..clean.len().min(400)]))?;

    let str_field = |key: &str| v[key].as_str().unwrap_or("").to_string();
    let str_arr = |key: &str| -> Vec<String> {
        v[key].as_array().map(|a| {
            a.iter().filter_map(|x| x.as_str()).map(String::from).collect()
        }).unwrap_or_default()
    };

    let genre_data = GenreData {
        generated: chrono::Utc::now().to_rfc3339(),
        industry: IndustryGenre {
            ebook: str_field("industry_ebook"),
            print: str_field("industry_print"),
        },
        kdp: KdpCategories {
            ebook: str_arr("kdp_ebook"),
            print: str_arr("kdp_print"),
        },
    };

    // Build human-readable markdown from the same JSON
    let now = chrono::Utc::now().format("%B %-d, %Y %H:%M UTC").to_string();
    let mut md = vec![
        "# Genre Analysis Report".to_string(),
        format!("Generated: {}  ", now),
        String::new(),
        "> **Note:** Genre classifications are based on the AI's training data (BISAC standards,".to_string(),
        "> Amazon KDP category knowledge, and publishing industry sources). KDP paths should be".to_string(),
        "> verified in Publisher Rocket. Comparable titles should be confirmed on Amazon.".to_string(),
        String::new(),
        "---".to_string(),
        String::new(),
        "## 1. Industry Genre Classification".to_string(),
        String::new(),
        "### Ebook".to_string(),
        format!("**{}**", str_field("industry_ebook")),
        String::new(),
    ];

    let comps_e = str_arr("comps_ebook");
    if !comps_e.is_empty() {
        md.push("**Comparable titles:**".to_string());
        for c in &comps_e { md.push(format!("- {}", c)); }
        md.push(String::new());
    }
    md.push(format!("**Reader demographic:** {}", str_field("reader_demographic")));
    md.push(String::new());
    md.push("### Print".to_string());
    md.push(format!("**{}**", str_field("industry_print")));
    md.push(String::new());
    md.push(format!("**Bookstore shelving:** {}", str_field("bookstore_shelving")));
    md.push(String::new());

    let comps_p = str_arr("comps_print");
    if !comps_p.is_empty() {
        md.push("**Comparable titles:**".to_string());
        for c in &comps_p { md.push(format!("- {}", c)); }
        md.push(String::new());
    }

    md.push("---".to_string());
    md.push(String::new());
    md.push("## 2. KDP Category Recommendations".to_string());
    md.push(String::new());
    md.push("### Kindle Ebook".to_string());
    for p in &genre_data.kdp.ebook { md.push(format!("- `{}`", p)); }
    md.push(String::new());
    md.push("### KDP Print".to_string());
    for p in &genre_data.kdp.print { md.push(format!("- `{}`", p)); }
    md.push(String::new());
    md.push("---".to_string());
    md.push(String::new());
    md.push("## 3. Genre Signals Summary".to_string());
    md.push(String::new());
    md.push(str_field("genre_signals"));
    md.push(String::new());
    md.push("---".to_string());
    md.push(String::new());
    md.push("## 4. Marketing Notes".to_string());
    md.push(String::new());
    for note in str_arr("marketing_notes") { md.push(format!("- {}", note)); }
    md.push(String::new());

    Ok((md.join("\n"), genre_data))
}

// ── File helpers ──────────────────────────────────────────────────────────────

fn load_summaries(summaries_dir: &Path) -> Vec<ChapterSummary> {
    let mut summaries = Vec::new();
    if !summaries_dir.exists() { return summaries; }
    let mut entries: Vec<_> = fs::read_dir(summaries_dir)
        .map(|r| r.flatten().collect())
        .unwrap_or_default();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if path.extension().map(|e| e == "json").unwrap_or(false) {
            if let Ok(raw) = fs::read_to_string(&path) {
                if let Ok(s) = serde_json::from_str::<ChapterSummary>(&raw) {
                    summaries.push(s);
                }
            }
        }
    }
    summaries
}

fn load_genre_data(path: &Path) -> Option<GenreData> {
    fs::read_to_string(path).ok()
        .and_then(|s| serde_json::from_str::<GenreData>(&s).ok())
}

fn collect_chapters(folder: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_md_recursive(folder, &mut files);
    files.sort_by(|a, b| {
        natural_sort_key(a.to_string_lossy().as_ref())
            .cmp(&natural_sort_key(b.to_string_lossy().as_ref()))
    });
    files
}

fn collect_md_recursive(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if name.starts_with('.') || name == "_analysis" { continue; }
        if path.is_dir() { collect_md_recursive(&path, out); }
        else if path.extension().map(|e| e == "md").unwrap_or(false) { out.push(path); }
    }
}

fn natural_sort_key(s: &str) -> Vec<u64> {
    let mut key = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() { cur.push(c); }
        else {
            if !cur.is_empty() { key.push(cur.parse::<u64>().unwrap_or(0)); cur.clear(); }
            key.push(c as u64);
        }
    }
    if !cur.is_empty() { key.push(cur.parse::<u64>().unwrap_or(0)); }
    key
}

fn extract_title(content: &str) -> Option<String> {
    content.lines().take(10)
        .find(|l| l.trim().starts_with("# "))
        .map(|l| l.trim().trim_start_matches("# ").trim().to_string())
}

fn truncate_words(text: &str, max: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() <= max { return text.to_string(); }
    words[..max].join(" ") + "\n\n[Truncated]"
}

fn build_combined_context(summaries: &[ChapterSummary]) -> String {
    summaries.iter().enumerate().map(|(i, s)| {
        format!("--- Chapter {} ({}, ~{} words) ---\n{}\n\n",
            i + 1, s.title, s.word_count, s.signals)
    }).collect()
}

fn sanitize(name: &str) -> String {
    name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

fn emit(app: &AppHandle, msg: &str) { let _ = app.emit("genre:log", msg); }

fn err(msg: &str) -> GenreResult {
    GenreResult { success: false, report: String::new(), error: msg.to_string() }
}
