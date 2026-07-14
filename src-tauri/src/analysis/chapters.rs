// analysis/chapters.rs — Phase 1: chapter-by-chapter summarization
//
// Collects .md files from a manuscript folder, sends each to an LLM for
// genre-signal extraction, and persists the results to SQLite.

use std::path::{Path, PathBuf};
use std::fs;
use tauri::{AppHandle, Manager};

use super::{emit, err, GenreResult, FolderRequest};
use crate::commands::call_llm;
use crate::db;

// ── Tauri command ────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn generate_summaries(app: AppHandle, request: FolderRequest) -> GenreResult {
    let folder = PathBuf::from(&request.folder);
    if !folder.exists() { return err("Folder does not exist."); }

    crate::reset_cancel();
    let chapters = collect_chapters(&folder);
    if chapters.is_empty() { return err("No .md files found."); }

    emit(&app, &format!("Found {} chapter file(s). Starting summaries...", chapters.len()));

    let database = app.state::<db::Db>();
    let (done, skipped) = phase1_summaries(&app, &database, &chapters, &request.folder, &request.provider, &request.api_key, &request.model).await;

    GenreResult {
        success: true,
        report:  format!("\u{2713} {} summarized, {} already done.", done, skipped),
        error:   String::new(),
    }
}

// ── Phase 1 implementation ───────────────────────────────────────────────────

pub(crate) async fn phase1_summaries(
    app: &AppHandle,
    database: &db::Db,
    chapters: &[PathBuf],
    story_folder: &str,
    provider: &str,
    api_key: &str,
    model: &str,
) -> (usize, usize) {
    let mut done = 0usize;
    let mut skipped = 0usize;

    for (i, chapter_path) in chapters.iter().enumerate() {
        let fname = chapter_path.file_name().unwrap_or_default().to_string_lossy().to_string();

        let already_done = {
            let conn = database.0.lock().unwrap();
            db::chapter_summary_exists(&conn, story_folder, &fname)
        };
        if already_done {
            emit(app, &format!("  [{}/{}] SKIP: {}", i + 1, chapters.len(), fname));
            skipped += 1;
            continue;
        }

        emit(app, &format!("  [{}/{}] Summarizing: {}", i + 1, chapters.len(), fname));

        let content = match fs::read_to_string(chapter_path) {
            Ok(c) if !c.trim().is_empty() => c,
            Ok(_)  => { emit(app, "    \u{26a0} Empty \u{2014} skipping."); continue; }
            Err(e) => { emit(app, &format!("    \u{26a0} Read error: {}", e)); continue; }
        };

        let word_count = content.split_whitespace().count();
        emit(app, &format!("    {} words", word_count));

        match summarize_chapter(provider, api_key, model, &fname, &truncate_words(&content, 8000)).await {
            Ok(signals) => {
                let title = extract_title(&content).unwrap_or_else(|| fname.clone());
                let conn = database.0.lock().unwrap();
                let _ = db::save_chapter_summary(&conn, story_folder, &fname, &title, &signals, word_count as i64);
                emit(app, &format!("    \u{2713} Done ({} signal chars)", signals.len()));
                done += 1;
            }
            Err(e) => emit(app, &format!("    \u{26a0} AI error: {}", e)),
        }

        if crate::is_cancelled() { emit(app, "\u{26a0} Cancelled."); break; }
    }

    emit(app, &format!("Phase 1 complete \u{2014} {} new, {} skipped.", done, skipped));
    (done, skipped)
}

// ── AI call ──────────────────────────────────────────────────────────────────

pub(crate) async fn summarize_chapter(
    provider: &str,
    api_key: &str,
    model: &str,
    filename: &str,
    content: &str,
) -> Result<String, String> {
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

    call_llm(
        provider, api_key, model, system,
        &format!("Chapter: {}\n\n---\n\n{}", filename, content),
        600,
    ).await
}

// ── File helpers (manuscript source files only — these stay on disk) ──────────

pub(crate) fn collect_chapters(folder: &Path) -> Vec<PathBuf> {
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
        if path.is_dir() {
            collect_md_recursive(&path, out);
        } else if path.extension().map(|e| e == "md").unwrap_or(false) {
            out.push(path);
        }
    }
}

fn natural_sort_key(s: &str) -> Vec<u64> {
    let mut key = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            cur.push(c);
        } else {
            if !cur.is_empty() {
                key.push(cur.parse::<u64>().unwrap_or(0));
                cur.clear();
            }
            key.push(c as u64);
        }
    }
    if !cur.is_empty() {
        key.push(cur.parse::<u64>().unwrap_or(0));
    }
    key
}

pub(crate) fn extract_title(content: &str) -> Option<String> {
    content.lines().take(10)
        .find(|l| l.trim().starts_with("# "))
        .map(|l| l.trim().trim_start_matches("# ").trim().to_string())
}

pub(crate) fn truncate_words(text: &str, max: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() <= max { return text.to_string(); }
    words[..max].join(" ") + "\n\n[Truncated]"
}

pub(crate) fn build_combined_context(summaries: &[db::ChapterSummaryRow]) -> String {
    summaries.iter().enumerate().map(|(i, s)| {
        format!("--- Chapter {} ({}, ~{} words) ---\n{}\n\n", i + 1, s.title, s.word_count, s.signals)
    }).collect()
}
