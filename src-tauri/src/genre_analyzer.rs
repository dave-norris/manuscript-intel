// genre_analyzer.rs — Full manuscript analysis pipeline
//
// Three commands:
//   generate_summaries  — Phase 1 only: chapter by chapter, saves JSON per chapter
//   analyze_genre       — Phase 2 only: reads summaries, produces genre-analysis.md
//                         + genre-data.json; auto-generates summaries if missing
//   run_full_analysis   — Phase 1 + 2: summaries + genre report
//                         PR competition data is handled separately by Analyze Competition

use std::path::{Path, PathBuf};
use std::fs;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tauri_plugin_dialog::DialogExt;

use crate::commands::call_anthropic;
use crate::models;

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ChapterSummary {
    pub file:       String,
    pub title:      String,
    pub signals:    String,
    pub word_count: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GenreData {
    pub generated: String,
    pub industry:  IndustryGenre,
    pub kdp:       KdpCategories,
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

        let (done, skipped) = phase1_summaries(&app, &chapters, &summaries_dir, &request.api_key);

        GenreResult {
            success: true,
            report:  format!("✓ {} summarized, {} already done.", done, skipped),
            error:   String::new(),
        }
    }).await.unwrap()
}

#[tauri::command]
pub async fn analyze_genre(app: AppHandle, request: FolderRequest) -> GenreResult {
    tokio::task::spawn_blocking(move || {
        let folder        = PathBuf::from(&request.folder);
        if !folder.exists() { return err("Folder does not exist."); }

        let analysis_dir  = folder.join("_analysis");
        let summaries_dir = analysis_dir.join("summaries");
        if let Err(e) = fs::create_dir_all(&summaries_dir) {
            return err(&format!("Cannot create _analysis/summaries: {}", e));
        }

        let mut summaries = load_summaries(&summaries_dir);

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


/// Run everything except folder selection and chapter summaries:
/// Analyze Genre → Full Analysis → Optimize Keywords → Generate PR Keywords
#[tauri::command]
pub async fn run_everything(app: AppHandle, request: FolderRequest) -> GenreResult {
    tokio::task::spawn_blocking(move || {
        let folder        = PathBuf::from(&request.folder);
        if !folder.exists() { return err("Folder does not exist."); }

        let analysis_dir  = folder.join("_analysis");
        let summaries_dir = analysis_dir.join("summaries");
        if let Err(e) = fs::create_dir_all(&summaries_dir) {
            return err(&format!("Cannot create _analysis/summaries: {}", e));
        }

        crate::reset_cancel();

        // ── Step 1: Ensure summaries exist ────────────────────────────────────
        let mut summaries = load_summaries(&summaries_dir);
        if summaries.is_empty() {
            emit(&app, "Step 1: No summaries found — generating now...");
            let chapters = collect_chapters(&folder);
            if chapters.is_empty() { return err("No .md chapter files found."); }
            phase1_summaries(&app, &chapters, &summaries_dir, &request.api_key);
            summaries = load_summaries(&summaries_dir);
            if summaries.is_empty() { return err("Could not produce chapter summaries."); }
        } else {
            emit(&app, &format!("Step 1: {} summaries found — skipping.", summaries.len()));
        }
        if crate::is_cancelled() { return err("Cancelled."); }

        // ── Step 2: Genre analysis ─────────────────────────────────────────────
        let data_path = analysis_dir.join("genre-data.json");
        emit(&app, "Step 2: Running genre analysis...");
        let genre_result = phase2_analyze(&app, &analysis_dir, &summaries, &request.api_key, &request.model);
        if !genre_result.success { return genre_result; }
        if crate::is_cancelled() { return err("Cancelled."); }

        // ── Step 3: Full report ────────────────────────────────────────────────
        emit(&app, "Step 3: Building full report...");
        let genre_data = match load_genre_data(&data_path) {
            Some(d) => d,
            None    => return err("genre-data.json missing after analysis."),
        };
        let now = chrono::Utc::now().format("%B %-d, %Y %H:%M UTC").to_string();
        let genre_report = fs::read_to_string(analysis_dir.join("genre-analysis.md")).unwrap_or_default();
        let full_report = vec![
            "# Full Genre & Market Analysis".to_string(),
            format!("Generated: {}", now),
            String::new(), "---".to_string(), String::new(),
            genre_report,
            String::new(), "---".to_string(), String::new(),
            "> Run **Analyze Competition** to add Publisher Rocket market data.".to_string(),
            String::new(),
        ].join("\n");
        let _ = fs::write(analysis_dir.join("full-report.md"), &full_report);
        emit(&app, "  ✓ full-report.md saved.");
        if crate::is_cancelled() { return err("Cancelled."); }

        // ── Step 4: Optimize KDP keywords ─────────────────────────────────────
        emit(&app, "Step 4: Optimizing KDP keywords...");
        let keywords_context = fs::read_to_string(analysis_dir.join("genre-analysis.md")).unwrap_or_default();
        let analysis_model = models::resolve_analysis_model(&request.model);
        match call_keyword_optimizer(&request.api_key, analysis_model, &genre_data, &keywords_context) {
            Ok(kw_md) => {
                let final_md = format!("*(Generated from genre analysis.)*\n\n{}", kw_md);
                let _ = fs::write(analysis_dir.join("kdp-keywords.md"), &final_md);
                emit(&app, "  ✓ kdp-keywords.md saved.");
            }
            Err(e) => emit(&app, &format!("  ⚠ Keyword optimization failed: {}", e)),
        }
        if crate::is_cancelled() { return err("Cancelled."); }

        // ── Step 5: Generate PR keywords ──────────────────────────────────────
        emit(&app, "Step 5: Generating PR Competition Analyzer keywords...");
        let pr_system = r#"You are a Publisher Rocket expert. Generate short search phrases for the Competition Analyzer tool.

Rules:
- 2-4 words maximum per phrase
- Plain English, no special characters
- Think like a reader browsing Amazon
- Include: genre combinations, setting descriptors, theme words, reader mood phrases

Return ONLY a JSON array of strings. No markdown, no preamble. Example:
["christian historical fiction", "first century rome", "faith romance clean"]"#;

        let pr_user = format!(
            "Book genre: {}\nKDP categories: {}\nGenre signals:\n{}",
            genre_data.industry.ebook,
            genre_data.kdp.ebook.iter()
                .map(|p| p.split('>').last().unwrap_or(p).trim().to_string())
                .collect::<Vec<_>>().join(", "),
            fs::read_to_string(analysis_dir.join("genre-analysis.md"))
                .map(|s| s[..s.len().min(500)].to_string())
                .unwrap_or_default()
        );

        match call_anthropic(&request.api_key, models::HAIKU, pr_system, &pr_user, 300) {
            Ok(raw) => {
                if let Some(clean) = extract_json_object(&raw) {
                    if let Ok(keywords) = serde_json::from_str::<Vec<String>>(&clean) {
                        if let Ok(json) = serde_json::to_string_pretty(&keywords) {
                            let _ = fs::write(analysis_dir.join("pr-keywords.json"), &json);
                            emit(&app, &format!("  ✓ pr-keywords.json saved ({} terms).", keywords.len()));
                            for kw in &keywords { emit(&app, &format!("    • {}", kw)); }
                        }
                    }
                } else {
                    emit(&app, "  ⚠ Could not parse PR keywords response.");
                }
            }
            Err(e) => emit(&app, &format!("  ⚠ PR keywords failed: {}", e)),
        }

        emit(&app, "✓ Analysis complete. Run Analyze Competition next.");

        // Return the genre analysis report for preview
        GenreResult { success: true, report: full_report, error: String::new() }
    }).await.unwrap()
}

#[tauri::command]
pub async fn run_full_analysis(app: AppHandle, request: FolderRequest) -> GenreResult {
    tokio::task::spawn_blocking(move || {
        let folder        = PathBuf::from(&request.folder);
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
        let data_path  = analysis_dir.join("genre-data.json");
        let genre_data = if data_path.exists() {
            emit(&app, "Phase 2: genre-data.json exists — loading...");
            match fs::read_to_string(&data_path).ok()
                .and_then(|s| serde_json::from_str::<GenreData>(&s).ok())
            {
                Some(d) => d,
                None => {
                    emit(&app, "  Could not parse genre-data.json — re-running Phase 2...");
                    match phase2_analyze(&app, &analysis_dir, &summaries, &request.api_key, &request.model) {
                        r if r.success => match load_genre_data(&data_path) {
                            Some(d) => d,
                            None    => return err("Phase 2 produced no genre-data.json"),
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
                    None    => return err("Phase 2 produced no genre-data.json"),
                },
                r => return r,
            }
        };

        emit(&app, &format!("  KDP ebook paths: {}", genre_data.kdp.ebook.join(", ")));
        emit(&app, &format!("  KDP print paths: {}", genre_data.kdp.print.join(", ")));

        // ── Build full report ─────────────────────────────────────────────────
        // PR competition data is added by Analyze Competition (uses CSV exports).
        emit(&app, "Building full report...");
        let now = chrono::Utc::now().format("%B %-d, %Y %H:%M UTC").to_string();

        let genre_report = fs::read_to_string(analysis_dir.join("genre-analysis.md"))
            .unwrap_or_default();

        let lines = vec![
            "# Full Genre & Market Analysis".to_string(),
            format!("Generated: {}", now),
            String::new(),
            "---".to_string(),
            String::new(),
            genre_report,
            String::new(),
            "---".to_string(),
            String::new(),
            "> Run **Analyze Competition** to add Publisher Rocket market data, category stats,".to_string(),
            "> competitor pricing, and cover analysis to this report.".to_string(),
            String::new(),
        ];

        let full_report = lines.join("\n");
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
    pub summary_count:   usize,
    pub has_genre_data:  bool,
    pub has_full_report: bool,
    pub has_keywords:    bool,
    pub has_pr_keywords: bool,
    pub has_competition: bool,
}

#[tauri::command]
pub async fn check_analysis_state(folder: String) -> AnalysisState {
    tokio::task::spawn_blocking(move || {
        let folder_path   = PathBuf::from(&folder);
        let analysis_dir  = folder_path.join("_analysis");
        let summaries_dir = analysis_dir.join("summaries");

        let summary_count = if summaries_dir.exists() {
            fs::read_dir(&summaries_dir)
                .map(|d| d.flatten()
                    .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
                    .count())
                .unwrap_or(0)
        } else { 0 };

        AnalysisState {
            has_folder:      folder_path.exists(),
            summary_count,
            has_genre_data:  analysis_dir.join("genre-data.json").exists(),
            has_full_report: analysis_dir.join("full-report.md").exists(),
            has_keywords:    analysis_dir.join("kdp-keywords.md").exists(),
            has_pr_keywords: analysis_dir.join("pr-keywords.json").exists(),
            has_competition: analysis_dir.join("competition-report.md").exists(),
        }
    }).await.unwrap()
}

// ── Keyword types ─────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct KeywordRequest {
    pub folder:  String,
    pub api_key: String,
    pub model:   String,
}

// ── PR Keyword Generator ──────────────────────────────────────────────────────

#[tauri::command]
pub async fn generate_pr_keywords(app: AppHandle, request: KeywordRequest) -> GenreResult {
    tokio::task::spawn_blocking(move || {
        let folder       = PathBuf::from(&request.folder);
        let analysis_dir = folder.join("_analysis");

        if !analysis_dir.exists() { return err("No _analysis folder. Run Analyze first."); }

        let genre_data = match load_genre_data(&analysis_dir.join("genre-data.json")) {
            Some(d) => d,
            None    => return err("genre-data.json not found. Run Analyze first."),
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
            "Book genre: {}\nKDP categories: {}\nGenre signals:\n{}",
            genre_data.industry.ebook,
            genre_data.kdp.ebook.iter()
                .map(|p| p.split('>').last().unwrap_or(p).trim().to_string())
                .collect::<Vec<_>>().join(", "),
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
                        let out_path = analysis_dir.join("pr-keywords.json");
                        if let Ok(json) = serde_json::to_string_pretty(&keywords) {
                            let _ = fs::write(&out_path, &json);
                        }
                        emit(&app, &format!("  ✓ {} PR search terms generated:", keywords.len()));
                        for kw in &keywords { emit(&app, &format!("    • {}", kw)); }

                        let now = chrono::Utc::now().format("%B %-d, %Y %H:%M UTC").to_string();
                        let mut md = vec![
                            "# PR Competition Analyzer Keywords".to_string(),
                            format!("Generated: {}", now),
                            String::new(),
                            "> These short phrases are for Publisher Rocket's Competition Analyzer.".to_string(),
                            "> They are NOT the same as your KDP keyword strings.".to_string(),
                            String::new(),
                        ];
                        for kw in &keywords { md.push(format!("- `{}`", kw)); }
                        md.push(String::new());

                        GenreResult { success: true, report: md.join("\n"), error: String::new() }
                    }
                }
            }
        }
    }).await.unwrap()
}

// ── Keyword Optimizer ─────────────────────────────────────────────────────────

#[tauri::command]
pub async fn optimize_keywords(app: AppHandle, request: KeywordRequest) -> GenreResult {
    tokio::task::spawn_blocking(move || {
        let folder       = PathBuf::from(&request.folder);
        let analysis_dir = folder.join("_analysis");

        if !analysis_dir.exists() { return err("No _analysis folder. Run Full Analysis first."); }

        let genre_data = match load_genre_data(&analysis_dir.join("genre-data.json")) {
            Some(d) => d,
            None    => return err("genre-data.json not found. Run Full Analysis first."),
        };

        let full_report = match fs::read_to_string(analysis_dir.join("full-report.md")) {
            Ok(s) => s,
            Err(_) => match fs::read_to_string(analysis_dir.join("genre-analysis.md")) {
                Ok(s) => s,
                Err(_) => return err("No report found. Run Analyze or Full Analysis first."),
            },
        };

        emit(&app, "Extracting keywords from report...");
        let keywords_text = extract_keyword_sections(&full_report);
        emit(&app, &format!("  Extracted {} chars of keyword material.", keywords_text.len()));

        let (keywords_context, source_note) = if keywords_text.is_empty() {
            emit(&app, "  No PR keyword sections — using genre signals.");
            let genre_signals = fs::read_to_string(analysis_dir.join("genre-analysis.md"))
                .unwrap_or_default();
            (genre_signals, "*(Generated from genre analysis. Run Analyze Competition for PR-sourced keywords.)*")
        } else {
            (keywords_text, "*(Generated from Publisher Rocket keyword data.)*")
        };

        let analysis_model = models::resolve_analysis_model(&request.model);
        emit(&app, &format!("Asking {} to optimize keywords...", analysis_model));

        match call_keyword_optimizer(&request.api_key, analysis_model, &genre_data, &keywords_context) {
            Err(e) => err(&format!("AI error: {}", e)),
            Ok(result_md) => {
                let final_md = format!("{}\n\n{}", source_note, result_md);
                let out_path = analysis_dir.join("kdp-keywords.md");
                let _ = fs::write(&out_path, &final_md);
                emit(&app, &format!("✓ Saved to {}", out_path.display()));
                GenreResult { success: true, report: final_md, error: String::new() }
            }
        }
    }).await.unwrap()
}

fn extract_keyword_sections(report: &str) -> String {
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
                out.push(String::new());
            } else if !trimmed.is_empty() {
                out.push(line.to_string());
            }
        }
    }
    out.join("\n")
}

fn call_keyword_optimizer(api_key: &str, model: &str, genre_data: &GenreData, keywords_text: &str)
    -> Result<String, String>
{
    let system = r#"You are an Amazon KDP keyword strategist helping an indie author maximize book discoverability.

Produce exactly 7 KDP keyword strings ready to paste into the KDP keyword fields.

Rules:
- Each string must be 50 characters or fewer (hard limit — count carefully)
- Natural search phrases a reader would actually type on Amazon
- Multi-word phrases only — Amazon already indexes your title and categories
- Do NOT repeat words already in the book's categories
- Vary the strings: setting, theme, reader mood, comp authors, tropes
- No punctuation except spaces and hyphens
- All lowercase

Return ONLY a JSON object:
{
  "keywords": [
    { "string": "the phrase", "chars": 10, "rationale": "one sentence why" },
    ... (exactly 7 items)
  ],
  "strategy": "One paragraph on the overall keyword strategy."
}"#;

    let user = format!(
        "Genre (ebook): {}\nGenre (print): {}\nKDP ebook categories: {}\nKDP print categories: {}\n\nKeyword material:\n\n{}",
        genre_data.industry.ebook,
        genre_data.industry.print,
        genre_data.kdp.ebook.join(", "),
        genre_data.kdp.print.join(", "),
        keywords_text
    );

    let raw = call_anthropic(api_key, model, system, &user, 1000)?;
    // Extract just the JSON object — ignore any trailing text or markdown fences
    let clean = extract_json_object(&raw)
        .ok_or_else(|| format!("No JSON object found in response: {}", &raw[..raw.len().min(200)]))?;

    let v: serde_json::Value = serde_json::from_str(&clean)
        .map_err(|e| format!("JSON parse: {} | got: {}", e, &clean[..clean.len().min(400)]))?;

    let keywords = v["keywords"].as_array().ok_or("Missing keywords array")?;
    let strategy = v["strategy"].as_str().unwrap_or("").to_string();

    let now = chrono::Utc::now().format("%B %-d, %Y %H:%M UTC").to_string();
    let mut md = vec![
        "# KDP Keyword Strings".to_string(),
        format!("Generated: {}", now),
        String::new(),
        "> These 7 strings are ready to paste directly into KDP's keyword fields.".to_string(),
        "> Each is 50 characters or fewer.".to_string(),
        String::new(),
        "---".to_string(),
        String::new(),
        "## Your 7 KDP Keyword Strings".to_string(),
        String::new(),
    ];

    for (i, kw) in keywords.iter().enumerate() {
        let s      = kw["string"].as_str().unwrap_or("");
        let chars  = kw["chars"].as_u64().unwrap_or(s.len() as u64);
        let why    = kw["rationale"].as_str().unwrap_or("");
        let actual = s.len();
        let flag   = if actual > 50 { " ⚠️ OVER 50 CHARS — shorten before using" } else { "" };
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
    md.push("4. Do NOT use commas inside a field".to_string());
    md.push("5. Do NOT repeat words already in your title, subtitle, or categories".to_string());
    md.push(String::new());

    Ok(md.join("\n"))
}

// ── Phase implementations ─────────────────────────────────────────────────────

fn phase1_summaries(app: &AppHandle, chapters: &[PathBuf], summaries_dir: &Path, api_key: &str)
    -> (usize, usize)
{
    let mut done = 0usize;
    let mut skipped = 0usize;

    for (i, chapter_path) in chapters.iter().enumerate() {
        let fname = chapter_path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let summary_path = summaries_dir.join(format!("{:03}-{}.json", i + 1, sanitize(&fname)));

        if summary_path.exists() {
            emit(app, &format!("  [{}/{}] SKIP: {}", i + 1, chapters.len(), fname));
            skipped += 1;
            continue;
        }

        emit(app, &format!("  [{}/{}] Summarizing: {}", i + 1, chapters.len(), fname));

        let content = match fs::read_to_string(chapter_path) {
            Ok(c) if !c.trim().is_empty() => c,
            Ok(_)  => { emit(app, "    ⚠ Empty — skipping."); continue; }
            Err(e) => { emit(app, &format!("    ⚠ Read error: {}", e)); continue; }
        };

        let word_count = content.split_whitespace().count();
        emit(app, &format!("    {} words", word_count));

        match summarize_chapter(api_key, models::HAIKU, &fname, &truncate_words(&content, 8000)) {
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

        if crate::is_cancelled() { emit(app, "⚠ Cancelled."); break; }
    }

    emit(app, &format!("Phase 1 complete — {} new, {} skipped.", done, skipped));
    (done, skipped)
}

fn phase2_analyze(app: &AppHandle, analysis_dir: &Path, summaries: &[ChapterSummary], api_key: &str, model: &str)
    -> GenreResult
{
    let analysis_model = models::resolve_analysis_model(model);
    let combined = build_combined_context(summaries);

    emit(app, &format!(
        "  Sending {} summaries ({} chars) to {}...",
        summaries.len(), combined.len(), analysis_model
    ));

    match call_ai_genre_analysis(api_key, analysis_model, &combined) {
        Err(e) => err(&format!("Phase 2 AI error: {}", e)),
        Ok((report_md, genre_data)) => {
            let _ = fs::write(analysis_dir.join("genre-analysis.md"), &report_md);
            emit(app, "  ✓ genre-analysis.md saved");
            if let Ok(json) = serde_json::to_string_pretty(&genre_data) {
                let _ = fs::write(analysis_dir.join("genre-data.json"), json);
                emit(app, "  ✓ genre-data.json saved");
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

fn call_ai_genre_analysis(api_key: &str, model: &str, combined: &str)
    -> Result<(String, GenreData), String>
{
    let system = r#"You are a senior publishing consultant specializing in Amazon KDP and the broader ebook/print marketplace.

Analyze the provided chapter genre-signal summaries and return a JSON object with this EXACT structure:

{
  "industry_ebook": "Primary genre / subgenre for ebook market",
  "industry_print": "Primary genre / subgenre for print market",
  "kdp_ebook": ["Full > Category > Path", "Second > Full > Path"],
  "kdp_print": ["Full > Category > Path", "Second > Full > Path"],
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

    let clean = extract_json_object(&raw)
        .ok_or_else(|| format!("No JSON object found: {}", &raw[..raw.len().min(200)]))?;

    let v: serde_json::Value = serde_json::from_str(&clean)
        .map_err(|e| format!("JSON parse: {} | got: {}", e, &clean[..clean.len().min(400)]))?;

    let str_field = |key: &str| v[key].as_str().unwrap_or("").to_string();
    let str_arr   = |key: &str| -> Vec<String> {
        v[key].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).map(String::from).collect())
              .unwrap_or_default()
    };

    let genre_data = GenreData {
        generated: chrono::Utc::now().to_rfc3339(),
        industry:  IndustryGenre { ebook: str_field("industry_ebook"), print: str_field("industry_print") },
        kdp:       KdpCategories {
            ebook: strip_kdp_paths(str_arr("kdp_ebook")),
            print: strip_kdp_paths(str_arr("kdp_print")),
        },
    };

    let now = chrono::Utc::now().format("%B %-d, %Y %H:%M UTC").to_string();
    let mut md = vec![
        "# Genre Analysis Report".to_string(),
        format!("Generated: {}", now),
        String::new(),
        "> **Note:** Classifications are based on AI training data. Verify KDP paths in Publisher Rocket.".to_string(),
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


/// Extract the first complete JSON object from a string.
/// Handles cases where the AI returns extra text before or after the JSON.
fn extract_json_object(text: &str) -> Option<String> {
    // Try the full text first (stripped of fences)
    let stripped = text.trim()
        .trim_start_matches("```json").trim_start_matches("```")
        .trim_end_matches("```").trim();
    if serde_json::from_str::<serde_json::Value>(stripped).is_ok() {
        return Some(stripped.to_string());
    }
    // Find first { and match to its closing }
    let start = text.find('{')?;
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    for (i, &b) in bytes[start..].iter().enumerate() {
        if escape { escape = false; continue; }
        match b {
            b'\\' if in_string => escape = true,
            b'"'  => in_string = !in_string,
            b'{'  if !in_string => depth += 1,
            b'}'  if !in_string => {
                depth -= 1;
                if depth == 0 {
                    let candidate = &text[start..start+i+1];
                    if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
                        return Some(candidate.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    None
}


/// Strip store-level prefixes from a KDP category path.
/// "Kindle Store > Kindle eBooks > Romance > Contemporary" → "Romance > Contemporary"
/// "Books > Literature & Fiction > Women's Fiction" → "Literature & Fiction > Women's Fiction"
fn strip_kdp_prefix(path: &str) -> String {
    let store_prefixes = [
        "kindle store > kindle ebooks > ",
        "kindle store > kindle books > ",
        "kindle store > ",
        "kindle ebooks > ",
        "kindle books > ",
        "books > ",
        "audible books & originals > ",
        "audible > ",
    ];
    let lower = path.to_lowercase();
    for prefix in &store_prefixes {
        if lower.starts_with(prefix) {
            return path[prefix.len()..].to_string();
        }
    }
    path.to_string()
}

fn strip_kdp_paths(paths: Vec<String>) -> Vec<String> {
    paths.into_iter().map(|p| strip_kdp_prefix(&p)).collect()
}

// ── File helpers ──────────────────────────────────────────────────────────────

fn load_summaries(summaries_dir: &Path) -> Vec<ChapterSummary> {
    if !summaries_dir.exists() { return Vec::new(); }
    let mut entries: Vec<_> = fs::read_dir(summaries_dir)
        .map(|r| r.flatten().collect()).unwrap_or_default();
    entries.sort_by_key(|e| e.file_name());
    entries.iter().filter_map(|entry| {
        let path = entry.path();
        if !path.extension().map(|e| e == "json").unwrap_or(false) { return None; }
        fs::read_to_string(&path).ok()
            .and_then(|raw| serde_json::from_str::<ChapterSummary>(&raw).ok())
    }).collect()
}

fn load_genre_data(path: &Path) -> Option<GenreData> {
    fs::read_to_string(path).ok().and_then(|s| serde_json::from_str::<GenreData>(&s).ok())
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
        format!("--- Chapter {} ({}, ~{} words) ---\n{}\n\n", i + 1, s.title, s.word_count, s.signals)
    }).collect()
}

fn sanitize(name: &str) -> String {
    name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

fn emit(app: &AppHandle, msg: &str) { let _ = app.emit("genre:log", msg); }

fn err(msg: &str) -> GenreResult {
    GenreResult { success: false, report: String::new(), error: msg.to_string() }
}
