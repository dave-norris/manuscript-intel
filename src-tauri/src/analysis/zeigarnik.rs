// analysis/zeigarnik.rs — Zeigarnik-effect proxy detector for the Craft platform.
//
// This is a deliberately AI-free, deterministic text scanner. The Zeigarnik
// effect itself — people recall interrupted tasks better than completed ones
// — is a claim about what happens in a reader's mind, which no algorithm can
// observe directly. What this module measures instead are textual proxies
// craft writers use to court that effect:
//
//   1. Chapter-ending tension  — does a chapter end on an unresolved beat
//      (a question, a cliffhanger phrase, an abrupt short fragment) or a
//      resolved one?
//   2. Open narrative questions — sentences that raise a question the prose
//      doesn't answer in the same breath.
//   3. Long-gap threads — a capitalized term/phrase (proxy for a named
//      person, place, or object) that appears, then goes quiet for several
//      chapters, then resurfaces. That gap is the shape of an open loop.
//
// None of this proves a reader will actually experience heightened recall —
// it surfaces where the manuscript's structure matches the pattern, so the
// author can judge for themselves. All phrase lists and thresholds are
// loaded from the zeigarnik_config table (see db.rs) rather than hardcoded.

use std::collections::HashMap;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};
use regex::Regex;

use super::{emit, err, GenreResult};
use super::chapters::{collect_chapters, extract_title};
use crate::db;

// ── Request / command ───────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct ZeigarnikRequest {
    pub folder: String,
}

#[tauri::command]
pub async fn analyze_zeigarnik_for_story(app: AppHandle, request: ZeigarnikRequest) -> GenreResult {
    let folder = PathBuf::from(&request.folder);
    if !folder.exists() { return err("Folder does not exist."); }

    crate::reset_cancel();
    let chapter_paths = collect_chapters(&folder);
    if chapter_paths.is_empty() { return err("No .md files found."); }

    let database = app.state::<db::Db>();
    let config = { let conn = database.0.lock().unwrap(); db::load_zeigarnik_config(&conn) };
    let run_ts = chrono::Utc::now().to_rfc3339();

    emit(&app, &format!("Found {} chapter file(s). Scanning for open loops (no AI — pattern matching only)...", chapter_paths.len()));

    // ── Pass 1: read + per-chapter metrics + raw text for entity scan ──────
    let mut chapter_texts: Vec<String> = Vec::with_capacity(chapter_paths.len());
    let mut chapter_rows: Vec<db::ZeigarnikChapterRow> = Vec::with_capacity(chapter_paths.len());

    for (i, path) in chapter_paths.iter().enumerate() {
        let fname = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let raw = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => { emit(&app, &format!("  ⚠ Could not read {}: {}", fname, e)); String::new() }
        };
        let title = extract_title(&raw).unwrap_or_else(|| fname.clone());
        let body = strip_markdown(&raw);
        let word_count = body.split_whitespace().count();
        let sentences = split_sentences(&body);
        let questions = extract_open_questions(&sentences, &config);
        let last_para = last_paragraph(&body);
        let (ending_type, tension_score, ending_snippet) = detect_ending(&last_para, &sentences, &config);

        emit(&app, &format!("  [{}/{}] {} — {} words, {} question(s), {} ending",
            i + 1, chapter_paths.len(), fname, word_count, questions.len(), ending_type));

        chapter_rows.push(db::ZeigarnikChapterRow {
            chapter_index: i as i64,
            file: fname,
            title,
            word_count: word_count as i64,
            sentence_count: sentences.len() as i64,
            question_count: questions.len() as i64,
            ending_type,
            tension_score: tension_score as i64,
            ending_snippet,
        });
        chapter_texts.push(body);

        if crate::is_cancelled() { emit(&app, "⚠ Cancelled."); return err("Cancelled."); }
    }

    // ── Pass 2: track candidate open-loop terms across chapters ────────────
    emit(&app, "Tracking recurring names, places, and objects across chapters...");
    let occurrences = scan_entity_occurrences(&chapter_texts);
    let threads = find_open_threads(&occurrences, &chapter_rows, &config);
    emit(&app, &format!("  {} candidate open thread(s) found (gap ≥ {} chapters).", threads.len(), config.min_gap_chapters_for_thread));

    // ── Persist ──────────────────────────────────────────────────────────
    {
        let conn = database.0.lock().unwrap();
        if let Err(e) = db::replace_zeigarnik_analysis(&conn, &request.folder, &chapter_rows, &threads) {
            return err(&format!("Could not save analysis: {}", e));
        }
    }

    // ── Summary + JSON document ─────────────────────────────────────────
    let total_chapters = chapter_rows.len();
    let total_words: i64 = chapter_rows.iter().map(|c| c.word_count).sum();
    let cliffhanger_count = chapter_rows.iter().filter(|c| c.ending_type == "cliffhanger").count();
    let resolved_count = chapter_rows.iter().filter(|c| c.ending_type == "resolved").count();
    let total_questions: i64 = chapter_rows.iter().map(|c| c.question_count).sum();
    let avg_tension = if total_chapters > 0 {
        chapter_rows.iter().map(|c| c.tension_score).sum::<i64>() as f64 / total_chapters as f64
    } else { 0.0 };
    let longest_gap_chapters = threads.iter().map(|t| t.max_gap_chapters).max().unwrap_or(0);

    let json = serde_json::json!({
        "schema": "zeigarnik_v1",
        "note": "Textual proxy analysis only — this measures manuscript structure (unresolved endings, open questions, long-gap threads), not reader recall itself. No AI was used to generate these results.",
        "summary": {
            "total_chapters": total_chapters,
            "total_words": total_words,
            "cliffhanger_endings": cliffhanger_count,
            "resolved_endings": resolved_count,
            "cliffhanger_pct": if total_chapters > 0 { (cliffhanger_count as f64 / total_chapters as f64 * 100.0).round() } else { 0.0 },
            "total_open_questions": total_questions,
            "avg_tension_score": (avg_tension * 10.0).round() / 10.0,
            "open_thread_count": threads.len(),
            "longest_gap_chapters": longest_gap_chapters,
        },
        "chapters": chapter_rows.iter().map(|c| serde_json::json!({
            "chapter_index": c.chapter_index,
            "file": c.file,
            "title": c.title,
            "word_count": c.word_count,
            "sentence_count": c.sentence_count,
            "question_count": c.question_count,
            "ending_type": c.ending_type,
            "tension_score": c.tension_score,
            "ending_snippet": c.ending_snippet,
        })).collect::<Vec<_>>(),
        "threads": threads.iter().map(|t| serde_json::json!({
            "term": t.term,
            "mention_count": t.mention_count,
            "first_chapter_index": t.first_chapter_index,
            "first_file": t.first_file,
            "first_snippet": t.first_snippet,
            "gap_start_index": t.gap_start_index,
            "gap_end_index": t.gap_end_index,
            "max_gap_chapters": t.max_gap_chapters,
            "max_gap_words": t.max_gap_words,
        })).collect::<Vec<_>>(),
    });
    let content = json.to_string();

    { let conn = database.0.lock().unwrap(); let _ = db::save_document_at(&conn, &request.folder, "zeigarnik_analysis", &content, &run_ts); }
    emit(&app, "✓ Zeigarnik analysis saved to database.");

    GenreResult { success: true, report: content, error: String::new(), run_ts }
}

// ── Text prep ────────────────────────────────────────────────────────────────

/// Strip the leading "# Title" line and light markdown formatting so pattern
/// matching runs against plain prose.
fn strip_markdown(raw: &str) -> String {
    raw.lines()
        .filter(|l| !l.trim_start().starts_with("# "))
        .collect::<Vec<_>>()
        .join("\n")
        .replace("**", "")
        .replace("__", "")
}

fn last_paragraph(body: &str) -> String {
    body.split("\n\n")
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .last()
        .unwrap_or("")
        .to_string()
}

/// Naive sentence splitter: good enough for pattern scanning, not linguistics.
/// Splits on ./!/? runs followed by whitespace, keeping the terminator.
fn split_sentences(body: &str) -> Vec<String> {
    let re = Regex::new(r"[^.!?]*[.!?]+[\)\]\u{201d}'\u{2019}\u{201c}\u{22}]*").unwrap();
    let mut out: Vec<String> = re.find_iter(body)
        .map(|m| m.as_str().trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    // Catch a trailing fragment with no terminal punctuation (common at the
    // very end of a chapter file).
    let matched_len: usize = out.iter().map(|s| s.len()).sum::<usize>();
    let remainder = body.trim();
    if remainder.len() > matched_len + 20 {
        if let Some(tail) = remainder.rsplit(['.', '!', '?']).next() {
            let tail = tail.trim();
            if tail.split_whitespace().count() >= 3 {
                out.push(tail.to_string());
            }
        }
    }
    out
}

// ── Chapter-ending tension scoring ──────────────────────────────────────────

fn detect_ending(last_para: &str, sentences: &[String], config: &db::ZeigarnikConfig) -> (String, i64, String) {
    let lower = last_para.to_lowercase();
    let mut score: i64 = 35; // neutral baseline

    let last_sentence = sentences.last().cloned().unwrap_or_default();
    let last_lower = last_sentence.to_lowercase();

    if last_lower.trim_end().ends_with('?') {
        score += 25;
    }
    if last_para.trim_end().ends_with('\u{2014}') || last_para.trim_end().ends_with("...") || last_para.trim_end().ends_with('\u{2026}') {
        score += 15;
    }
    if config.cliffhanger_markers.iter().any(|m| lower.contains(m.as_str())) {
        score += 25;
    }
    if config.resolution_markers.iter().any(|m| lower.contains(m.as_str())) {
        score -= 35;
    }
    if sentences.len() >= 2 {
        let last_words = last_sentence.split_whitespace().count();
        let prior_words = sentences[sentences.len() - 2].split_whitespace().count();
        if last_words > 0 && last_words <= config.short_fragment_max_words && prior_words > last_words * 2 {
            score += 12;
        }
    }

    let score = score.clamp(0, 100);
    let ending_type = if score >= 65 { "cliffhanger" } else if score <= 25 { "resolved" } else { "neutral" };

    // Snippet: last one or two sentences, capped.
    let take = if sentences.len() >= 2 { 2 } else { sentences.len().min(1) };
    let mut snippet = sentences[sentences.len().saturating_sub(take)..].join(" ");
    if snippet.is_empty() { snippet = last_para.chars().take(200).collect(); }
    if snippet.chars().count() > 240 {
        snippet = snippet.chars().take(240).collect::<String>() + "…";
    }

    (ending_type.to_string(), score, snippet)
}

// ── Open narrative questions ────────────────────────────────────────────────

fn extract_open_questions(sentences: &[String], config: &db::ZeigarnikConfig) -> Vec<String> {
    // Collect every question sentence long enough to be more than a dialogue
    // tag, then prefer ones that read like a genuine open narrative question
    // ("What if...", "Why would...") over throwaway quips ("You okay?") when
    // trimming down to the per-chapter cap. Order within each group is
    // preserved (stable sort) so earlier questions still come first.
    let mut candidates: Vec<(bool, String)> = Vec::new();
    for s in sentences {
        if !s.trim_end().ends_with('?') { continue; }
        let word_count = s.split_whitespace().count();
        if word_count < config.min_question_words { continue; }
        let lower = s.to_lowercase();
        let is_lead_in = config.question_lead_ins.iter().any(|p| lower.contains(p.as_str()));
        candidates.push((is_lead_in, s.trim().to_string()));
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    candidates.into_iter().take(config.max_questions_per_chapter).map(|(_, s)| s).collect()
}

// ── Entity / recurring-term tracking (for long-gap open threads) ───────────

struct Occurrence {
    chapter_idx: usize,
    sentence_initial: bool,
    snippet: String,
}

/// Find 1–3-word capitalized sequences (proxy for proper nouns / named
/// objects) in each chapter, noting whether each occurrence sits at the
/// start of a sentence (where capitalization is just grammar, not a name).
fn scan_entity_occurrences(chapter_texts: &[String]) -> HashMap<String, Vec<Occurrence>> {
    let cap_word = r"[A-Z][a-zA-Z'\u{2019}]+";
    let re = Regex::new(&format!(r"(?:{cw})(?:\s+(?:{cw})){{0,2}}", cw = cap_word)).unwrap();
    let stopwords: [&str; 24] = [
        "The", "A", "An", "He", "She", "It", "They", "We", "I", "But", "And", "So",
        "Then", "When", "If", "As", "You", "Chapter", "There", "Here", "This", "That", "Her", "His",
    ];

    let mut map: HashMap<String, Vec<Occurrence>> = HashMap::new();

    for (chapter_idx, text) in chapter_texts.iter().enumerate() {
        for m in re.find_iter(text) {
            let raw = m.as_str();
            let words: Vec<&str> = raw.split_whitespace().collect();
            let is_multiword = words.len() > 1;

            if !is_multiword && stopwords.contains(&raw) { continue; }

            // Sentence-initial if the preceding non-space char is a sentence
            // terminator, a quote, or we're at the start of the text.
            let start = m.start();
            let sentence_initial = {
                let before = text[..start].trim_end();
                match before.chars().last() {
                    None => true,
                    Some(c) => matches!(c, '.' | '!' | '?' | '\u{201c}' | '\u{2018}' | '"'),
                }
            };

            let norm = raw.to_lowercase();
            let ctx_start = floor_char_boundary(text, start.saturating_sub(40));
            let ctx_end = ceil_char_boundary(text, (m.end() + 40).min(text.len()));
            let snippet = format!("…{}…", text[ctx_start..ctx_end].trim());

            map.entry(norm).or_default().push(Occurrence { chapter_idx, sentence_initial, snippet });
        }
    }

    map
}

fn find_open_threads(
    occurrences: &HashMap<String, Vec<Occurrence>>,
    chapter_rows: &[db::ZeigarnikChapterRow],
    config: &db::ZeigarnikConfig,
) -> Vec<db::ZeigarnikThreadRow> {
    let mut threads = Vec::new();

    for (term, occs) in occurrences {
        if term.len() < config.min_thread_term_len { continue; }

        let is_multiword = term.contains(' ');
        let has_non_initial = occs.iter().any(|o| !o.sentence_initial);
        // Single capitalized words that only ever appear at a sentence start
        // are almost always just grammar (start-of-sentence capitalization),
        // not a genuine recurring name/object — skip them.
        if !is_multiword && !has_non_initial { continue; }

        let mut chapter_idxs: Vec<usize> = occs.iter().map(|o| o.chapter_idx).collect();
        chapter_idxs.sort_unstable();
        chapter_idxs.dedup();

        if chapter_idxs.len() < 2 { continue; }
        if chapter_idxs.len() > config.max_total_mentions_for_thread { continue; } // ubiquitous — a main character, not an open loop

        let mut max_gap = 0usize;
        let mut gap_start = 0usize;
        let mut gap_end = 0usize;
        for w in chapter_idxs.windows(2) {
            let g = w[1] - w[0];
            if g > max_gap { max_gap = g; gap_start = w[0]; gap_end = w[1]; }
        }
        if max_gap < config.min_gap_chapters_for_thread { continue; }

        let gap_words: i64 = chapter_rows.iter()
            .filter(|c| (c.chapter_index as usize) > gap_start && (c.chapter_index as usize) < gap_end)
            .map(|c| c.word_count)
            .sum();

        let first_occ = occs.iter().find(|o| o.chapter_idx == chapter_idxs[0]);
        let first_file = chapter_rows.get(chapter_idxs[0]).map(|c| c.file.clone()).unwrap_or_default();

        // Render with original casing from the first occurrence's snippet where possible.
        let display_term = title_case(term);

        threads.push(db::ZeigarnikThreadRow {
            term: display_term,
            mention_count: chapter_idxs.len() as i64,
            first_chapter_index: chapter_idxs[0] as i64,
            first_file,
            first_snippet: first_occ.map(|o| o.snippet.clone()).unwrap_or_default(),
            gap_start_index: gap_start as i64,
            gap_end_index: gap_end as i64,
            max_gap_chapters: max_gap as i64,
            max_gap_words: gap_words,
        });
    }

    threads.sort_by(|a, b| {
        b.max_gap_chapters.cmp(&a.max_gap_chapters)
            .then(b.max_gap_words.cmp(&a.max_gap_words))
    });
    threads.truncate(config.top_threads_limit);
    threads
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) { i -= 1; }
    i
}

fn ceil_char_boundary(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) { i += 1; }
    i
}

fn title_case(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
