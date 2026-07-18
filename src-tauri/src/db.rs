// db.rs — SQLite-backed storage for the genre/category reference system.
//
// Source of truth for: genres, KDP category paths, genre<->category links,
// and per-story genre rankings / category-finder results. Story registration
// (stories.json) and the human-readable .md reports stay as files — those are
// meant to be read directly outside the app. The DB is queried to produce
// those reports, not the other way around.
//
// The genre-list.json / genre-kdp-map.json files in src-tauri/data/ are used
// ONLY as one-time seed data on first launch (when the genres table is
// empty). After that, the database is authoritative — new categories
// discovered via Category Finder get written straight into it, so the
// genre-to-KDP-path map grows on its own with real, verified data instead of
// staying frozen at the hand-typed seed set.

use rusqlite::{params, Connection};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

const SEED_GENRE_LIST_JSON:    &str = include_str!("../data/genre-list.json");
const SEED_GENRE_KDP_MAP_JSON: &str = include_str!("../data/genre-kdp-map.json");
const SEED_BISAC_JSON:         &str = include_str!("../data/bisac-fiction.json");
const SEED_ZEIGARNIK_CONFIG_JSON: &str = include_str!("../data/zeigarnik-config.json");

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS genres (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL UNIQUE,
    description TEXT
);

CREATE TABLE IF NOT EXISTS kdp_categories (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    path           TEXT NOT NULL,
    store          TEXT NOT NULL DEFAULT 'Kindle',
    amazon_node_id TEXT,
    source         TEXT NOT NULL DEFAULT 'manual',   -- 'manual' | 'winningcat' | 'category_finder'
    verified_at    TEXT,                              -- last time confirmed live
    created_at     TEXT NOT NULL,
    UNIQUE(path, store)
);

CREATE TABLE IF NOT EXISTS genre_kdp_links (
    genre_id    INTEGER NOT NULL REFERENCES genres(id) ON DELETE CASCADE,
    category_id INTEGER NOT NULL REFERENCES kdp_categories(id) ON DELETE CASCADE,
    PRIMARY KEY (genre_id, category_id)
);

CREATE TABLE IF NOT EXISTS genre_rankings (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    story_folder TEXT NOT NULL,
    genre_id     INTEGER NOT NULL REFERENCES genres(id),
    confidence   INTEGER NOT NULL,
    reason       TEXT,
    generated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS category_results (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    story_folder  TEXT NOT NULL,
    category_id   INTEGER REFERENCES kdp_categories(id),
    raw_path      TEXT NOT NULL,
    store         TEXT NOT NULL,
    confidence    INTEGER NOT NULL,
    sales_to_one  TEXT,
    sales_to_ten  TEXT,
    publisher_pct TEXT,
    ku_pct        TEXT,
    status        TEXT NOT NULL,   -- 'matched' | 'considered' | 'failed'
    note          TEXT,
    generated_at  TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_rankings_folder  ON genre_rankings(story_folder);
CREATE INDEX IF NOT EXISTS idx_results_folder   ON category_results(story_folder);
CREATE INDEX IF NOT EXISTS idx_categories_path  ON kdp_categories(path);
CREATE INDEX IF NOT EXISTS idx_categories_store ON kdp_categories(store);

CREATE TABLE IF NOT EXISTS chapter_summaries (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    story_folder TEXT NOT NULL,
    file         TEXT NOT NULL,
    title        TEXT,
    signals      TEXT,
    word_count   INTEGER,
    updated_at   TEXT NOT NULL,
    UNIQUE(story_folder, file)
);

CREATE TABLE IF NOT EXISTS genre_data (
    story_folder         TEXT PRIMARY KEY,
    generated_at         TEXT NOT NULL,
    industry_ebook       TEXT,
    industry_print       TEXT,
    genre_signals        TEXT,
    reader_demographic   TEXT,
    bookstore_shelving   TEXT,
    kdp_ebook_json       TEXT NOT NULL DEFAULT '[]',
    kdp_print_json       TEXT NOT NULL DEFAULT '[]',
    comps_ebook_json     TEXT NOT NULL DEFAULT '[]',
    comps_print_json     TEXT NOT NULL DEFAULT '[]',
    marketing_notes_json TEXT NOT NULL DEFAULT '[]'
);

CREATE TABLE IF NOT EXISTS kdp_keywords (
    story_folder TEXT PRIMARY KEY,
    generated_at TEXT NOT NULL,
    keywords_json TEXT NOT NULL,   -- [{"string":..,"chars":..,"rationale":..}]
    strategy     TEXT,
    source_note  TEXT
);

CREATE TABLE IF NOT EXISTS mi_search_terms (
    story_folder  TEXT PRIMARY KEY,
    generated_at  TEXT NOT NULL,
    keywords_json TEXT NOT NULL   -- ["kw1","kw2",...]
);

CREATE TABLE IF NOT EXISTS discovery_keywords (
    story_folder  TEXT PRIMARY KEY,
    generated_at  TEXT NOT NULL,
    keywords_json TEXT NOT NULL   -- [{"phrase":..,"rationale":..}]
);

CREATE TABLE IF NOT EXISTS keyword_search_results (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    story_folder  TEXT NOT NULL,
    seed          TEXT NOT NULL,
    keyword       TEXT NOT NULL,
    searches      TEXT,
    competition   TEXT,
    earnings      TEXT,
    generated_at  TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_keyword_results_folder ON keyword_search_results(story_folder);

-- Rendered markdown cache for the Reports panel. Every report type is
-- re-rendered fresh from its structured source table whenever regenerated —
-- this table is what the UI reads, never hand-edited, never stale-checked
-- against a file that quietly stopped being written (see: the genre-ranking
-- .json/.md drift this replaces).
CREATE TABLE IF NOT EXISTS story_documents (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    story_folder TEXT NOT NULL,
    doc_type     TEXT NOT NULL,
    content      TEXT NOT NULL,
    generated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_story_docs_folder ON story_documents(story_folder, doc_type);

CREATE INDEX IF NOT EXISTS idx_summaries_folder ON chapter_summaries(story_folder);

CREATE TABLE IF NOT EXISTS bisac_codes (
    code    TEXT PRIMARY KEY,
    heading TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS bisac_classifications (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    story_folder TEXT NOT NULL,
    code         TEXT NOT NULL,
    heading      TEXT NOT NULL,
    confidence   INTEGER NOT NULL,
    reason       TEXT,
    generated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_bisac_folder ON bisac_classifications(story_folder);

-- Versioned saved reports. The user explicitly saves a report version via the
-- UI. Each save auto-increments the version number per story+doc_type pair.
-- Reports panel shows saved versions newest-first.
CREATE TABLE IF NOT EXISTS saved_reports (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    story_folder TEXT NOT NULL,
    doc_type     TEXT NOT NULL,
    version      INTEGER NOT NULL,
    label        TEXT NOT NULL,
    content      TEXT NOT NULL,
    saved_at     TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_saved_reports_folder ON saved_reports(story_folder, doc_type);

CREATE TABLE IF NOT EXISTS report_types (
    id           TEXT PRIMARY KEY,
    label        TEXT NOT NULL,
    description  TEXT NOT NULL,
    platforms    TEXT NOT NULL DEFAULT 'kdp,wide',  -- comma-separated: 'kdp', 'wide', or 'kdp,wide'
    depends_on   TEXT NOT NULL DEFAULT ''           -- comma-separated report_type ids
);

-- Zeigarnik effect detector: pure textual-proxy analysis, no AI. Phrase lists
-- and thresholds live here (seeded once from zeigarnik-config.json) instead of
-- being hardcoded in Rust, so they can be tuned per-project without a rebuild.
CREATE TABLE IF NOT EXISTS zeigarnik_config (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL   -- JSON-encoded: array of strings, or a single number/object
);

-- One row per manuscript chapter per analysis run (replaced wholesale each run).
CREATE TABLE IF NOT EXISTS zeigarnik_chapters (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    story_folder   TEXT NOT NULL,
    chapter_index  INTEGER NOT NULL,
    file           TEXT NOT NULL,
    title          TEXT NOT NULL,
    word_count     INTEGER NOT NULL,
    sentence_count INTEGER NOT NULL,
    question_count INTEGER NOT NULL,
    ending_type    TEXT NOT NULL,   -- 'cliffhanger' | 'neutral' | 'resolved'
    tension_score  INTEGER NOT NULL, -- 0-100, heuristic
    ending_snippet TEXT NOT NULL,
    generated_at   TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_zeigarnik_chapters_folder ON zeigarnik_chapters(story_folder);

-- Candidate open loops: a capitalized term/phrase that reappears after a
-- long gap of chapters. A textual proxy for "unresolved thread the reader
-- may still be holding open" — not a direct measurement of recall.
CREATE TABLE IF NOT EXISTS zeigarnik_threads (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    story_folder       TEXT NOT NULL,
    term               TEXT NOT NULL,
    mention_count      INTEGER NOT NULL,
    first_chapter_index INTEGER NOT NULL,
    first_file         TEXT NOT NULL,
    first_snippet      TEXT NOT NULL,
    gap_start_index    INTEGER NOT NULL,  -- chapter after which the term went quiet
    gap_end_index      INTEGER NOT NULL,  -- chapter where it resurfaces
    max_gap_chapters   INTEGER NOT NULL,
    max_gap_words      INTEGER NOT NULL,
    generated_at       TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_zeigarnik_threads_folder ON zeigarnik_threads(story_folder);

-- Continuity Checker: groups stories into a series (reading order) so facts
-- can be compared across books, not just within one manuscript.
CREATE TABLE IF NOT EXISTS series (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    name       TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS series_books (
    series_id    INTEGER NOT NULL REFERENCES series(id) ON DELETE CASCADE,
    story_folder TEXT NOT NULL,
    story_name   TEXT NOT NULL DEFAULT '',
    book_order   INTEGER NOT NULL,
    PRIMARY KEY (series_id, story_folder)
);

CREATE INDEX IF NOT EXISTS idx_series_books_series ON series_books(series_id);

-- AI-extracted continuity-relevant facts, one row per (entity, attribute,
-- chapter) triple. Extraction runs once per chapter; comparison (finding
-- contradictions) is a separate pass over the accumulated facts — see
-- continuity_findings below.
CREATE TABLE IF NOT EXISTS continuity_facts (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    story_folder  TEXT NOT NULL,
    chapter_index INTEGER NOT NULL,
    file          TEXT NOT NULL,
    chapter_title TEXT NOT NULL,
    entity        TEXT NOT NULL,
    entity_type   TEXT NOT NULL,   -- 'character' | 'place' | 'object' | 'timeline' | 'other'
    attribute     TEXT NOT NULL,
    value         TEXT NOT NULL,
    snippet       TEXT NOT NULL,
    generated_at  TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_continuity_facts_folder ON continuity_facts(story_folder);

-- AI-judged contradictions. `scope` is 'manuscript' (scope_key = story_folder)
-- or 'series' (scope_key = 'series:<id>') so the same table serves both.
CREATE TABLE IF NOT EXISTS continuity_findings (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    scope            TEXT NOT NULL,
    scope_key        TEXT NOT NULL,
    entity           TEXT NOT NULL,
    attribute        TEXT NOT NULL,
    verdict          TEXT NOT NULL,   -- 'contradiction' | 'possible' | 'likely_intentional'
    confidence       INTEGER NOT NULL,
    explanation      TEXT NOT NULL,
    occurrences_json TEXT NOT NULL,
    generated_at     TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_continuity_findings_scope ON continuity_findings(scope, scope_key);

-- Prompt template system: editable AI prompts stored in DB instead of hardcoded.
-- Each report function has a system prompt and a user prompt template with {placeholders}.
CREATE TABLE IF NOT EXISTS prompt_templates (
    id              TEXT PRIMARY KEY,    -- e.g. 'continuity_extract', 'sdt_check'
    label           TEXT NOT NULL,
    system_prompt   TEXT NOT NULL,
    user_template   TEXT NOT NULL,       -- uses {chapter_title}, {chapter_text}, {bible}, etc.
    max_tokens      INTEGER NOT NULL DEFAULT 4000,
    json_mode       INTEGER NOT NULL DEFAULT 0,  -- 1 = force JSON response format
    version         INTEGER NOT NULL DEFAULT 1,
    updated_at      TEXT NOT NULL DEFAULT ''
);

-- Preprocessed chapter text, cached per report type. Invalidated when
-- the source file changes (via modified_at timestamp comparison).
CREATE TABLE IF NOT EXISTS preprocessed_chapters (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    story_folder   TEXT NOT NULL,
    chapter_file   TEXT NOT NULL,
    report_type    TEXT NOT NULL,        -- e.g. 'continuity_extract', 'sdt_check'
    processed_text TEXT NOT NULL,
    source_modified_at TEXT NOT NULL,    -- file mtime when preprocessed
    created_at     TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_preproc_unique ON preprocessed_chapters(story_folder, chapter_file, report_type);

-- Configurable story folder layout (scaffolded on Create empty story).
-- role: '' | 'manuscript' | 'bible' | 'characters'
CREATE TABLE IF NOT EXISTS folder_structure (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    path       TEXT NOT NULL,
    role       TEXT NOT NULL DEFAULT '',
    sort_order INTEGER NOT NULL DEFAULT 0
);
"#;

pub struct Db(pub Mutex<Connection>);

/// Open (or create) the app's SQLite database in the platform app-data
/// directory, apply schema, and seed from JSON on first run only.
pub fn init(app: &AppHandle) -> Result<Db, String> {
    let dir: PathBuf = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create app data dir: {}", e))?;

    let db_path = dir.join("manuscript-intel.db");
    let conn = Connection::open(&db_path).map_err(|e| format!("Cannot open database: {}", e))?;
    conn.execute_batch(SCHEMA).map_err(|e| format!("Schema error: {}", e))?;

    // Migration: bisac_classifications gained a `format` column (ebook/print)
    // after initial release. Ignore the error if it already exists.
    let _ = conn.execute("ALTER TABLE bisac_classifications ADD COLUMN format TEXT NOT NULL DEFAULT 'ebook'", []);

    // Migration: kdp_categories gained `last_seen_at` so re-importing an
    // updated WinningCat file can detect categories that dropped out of the
    // new file (retired/renamed by Amazon) instead of leaving stale rows
    // sitting in the catalog forever with no signal they're outdated.
    let _ = conn.execute("ALTER TABLE kdp_categories ADD COLUMN last_seen_at TEXT", []);

    // Migration: saved_reports table for versioned report storage.
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS saved_reports (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            story_folder TEXT NOT NULL,
            doc_type     TEXT NOT NULL,
            version      INTEGER NOT NULL,
            label        TEXT NOT NULL,
            content      TEXT NOT NULL,
            saved_at     TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_saved_reports_folder ON saved_reports(story_folder, doc_type);"
    );

    // Migration: move data from old pr_keywords table into mi_search_terms.
    // The schema already creates mi_search_terms, so we just copy any data and drop the old table.
    let _ = conn.execute_batch(
        "INSERT OR IGNORE INTO mi_search_terms (story_folder, generated_at, keywords_json)
         SELECT story_folder, generated_at, keywords_json FROM pr_keywords;
         DROP TABLE IF EXISTS pr_keywords;"
    );

    // Migration: story_documents from single-version (PRIMARY KEY on story_folder+doc_type)
    // to multi-version (auto-increment id). Recreate table if it lacks an id column.
    let has_id: bool = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('story_documents') WHERE name = 'id'",
        [], |r| r.get::<_, i64>(0)
    ).unwrap_or(0) > 0;

    if !has_id {
        let _ = conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS story_documents_new (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                story_folder TEXT NOT NULL,
                doc_type     TEXT NOT NULL,
                content      TEXT NOT NULL,
                generated_at TEXT NOT NULL
            );
            INSERT INTO story_documents_new (story_folder, doc_type, content, generated_at)
                SELECT story_folder, doc_type, content, generated_at FROM story_documents;
            DROP TABLE story_documents;
            ALTER TABLE story_documents_new RENAME TO story_documents;
            CREATE INDEX IF NOT EXISTS idx_story_docs_folder ON story_documents(story_folder, doc_type);"
        );
    }

    // Migration: series.bible_path column for series bible support.
    let _ = conn.execute("ALTER TABLE series ADD COLUMN bible_path TEXT NOT NULL DEFAULT ''", []);

    seed_if_empty(&conn)?;
    seed_bisac_if_empty(&conn)?;
    seed_report_types(&conn)?;
    seed_prompt_templates(&conn)?;
    seed_zeigarnik_config_if_empty(&conn)?;

    Ok(Db(Mutex::new(conn)))
}

fn seed_if_empty(conn: &Connection) -> Result<(), String> {
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM genres", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if count > 0 { return Ok(()); }

    #[derive(serde::Deserialize)]
    struct SeedGenre { name: String, description: String }

    let genres: Vec<SeedGenre> = serde_json::from_str(SEED_GENRE_LIST_JSON)
        .map_err(|e| format!("Cannot parse seed genre-list.json: {}", e))?;
    let kdp_map: std::collections::HashMap<String, Vec<String>> =
        serde_json::from_str(SEED_GENRE_KDP_MAP_JSON)
            .map_err(|e| format!("Cannot parse seed genre-kdp-map.json: {}", e))?;

    let now = chrono::Utc::now().to_rfc3339();

    for g in &genres {
        conn.execute(
            "INSERT OR IGNORE INTO genres (name, description) VALUES (?1, ?2)",
            params![g.name, g.description],
        ).map_err(|e| e.to_string())?;

        let genre_id: i64 = conn.query_row(
            "SELECT id FROM genres WHERE name = ?1", params![g.name], |r| r.get(0)
        ).map_err(|e| e.to_string())?;

        if let Some(paths) = kdp_map.get(&g.name) {
            for path in paths {
                conn.execute(
                    "INSERT OR IGNORE INTO kdp_categories (path, store, source, created_at)
                     VALUES (?1, 'Kindle', 'manual', ?2)",
                    params![path, now],
                ).map_err(|e| e.to_string())?;

                let category_id: i64 = conn.query_row(
                    "SELECT id FROM kdp_categories WHERE path = ?1 AND store = 'Kindle'",
                    params![path], |r| r.get(0)
                ).map_err(|e| e.to_string())?;

                conn.execute(
                    "INSERT OR IGNORE INTO genre_kdp_links (genre_id, category_id) VALUES (?1, ?2)",
                    params![genre_id, category_id],
                ).map_err(|e| e.to_string())?;
            }
        }
    }

    Ok(())
}

fn seed_bisac_if_empty(conn: &Connection) -> Result<(), String> {
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM bisac_codes", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if count > 0 { return Ok(()); }

    #[derive(serde::Deserialize)]
    struct SeedBisac { code: String, heading: String }

    let codes: Vec<SeedBisac> = serde_json::from_str(SEED_BISAC_JSON)
        .map_err(|e| format!("Cannot parse seed bisac-fiction.json: {}", e))?;

    for c in &codes {
        conn.execute(
            "INSERT OR IGNORE INTO bisac_codes (code, heading) VALUES (?1, ?2)",
            params![c.code, c.heading],
        ).map_err(|e| e.to_string())?;
    }

    Ok(())
}

fn seed_report_types(conn: &Connection) -> Result<(), String> {
    let rows: &[(&str, &str, &str, &str, &str)] = &[
        // (id, label, description, platforms, depends_on)
        ("chapter_summaries", "Chapter Summaries", "Extract genre signals from each chapter of the manuscript.", "kdp,wide,craft", ""),
        ("genre_analysis", "Genre Analysis", "Industry genre classification, KDP paths, comps, and reader demographic.", "kdp,wide", "chapter_summaries"),
        ("genre_ranking", "Genre Ranking", "Score the manuscript against all known genres independently.", "kdp,wide", "chapter_summaries,genre_analysis"),
        ("kdp_categories", "KDP Categories", "Find the best-fit Amazon categories with discoverability stats.", "kdp", "chapter_summaries,genre_analysis,genre_ranking"),
        ("kdp_keywords", "KDP Keywords", "Optimize the 7 keyword strings for KDP discoverability.", "kdp", "chapter_summaries,genre_analysis,genre_ranking"),
        ("bisac_classification", "BISAC Classification", "Select BISAC subject codes for KDP Print and Ingram distribution.", "kdp,wide", "chapter_summaries,genre_analysis"),
        ("mi_search_terms", "Search Terms", "Generate competition search phrases for market analysis.", "kdp", "chapter_summaries,genre_analysis"),
        ("discovery_keywords", "Discovery Keywords", "Keywords optimized for Apple Books, Kobo, Google Play, and SEO.", "wide", "chapter_summaries,genre_analysis"),
        ("analysis", "Full Analysis", "Combined report: categories, BISAC, keywords, and positioning all in one.", "kdp", "chapter_summaries,genre_analysis,genre_ranking,kdp_categories,kdp_keywords,bisac_classification,mi_search_terms"),
        ("keyword_search", "Keyword Search Results", "Amazon keyword volume and competition data from DataForSEO.", "kdp", "chapter_summaries,genre_analysis,genre_ranking"),
        ("competition_report", "Competition Analysis", "Market landscape: how competitive the niche is, who dominates.", "kdp", "mi_search_terms"),
        ("review_mining", "Reader Review Intelligence", "Reader insights extracted from competitor book reviews.", "kdp", "mi_search_terms"),
        ("author_analysis", "Competitor Author Analysis", "Competitor pricing, release cadence, and series strategy.", "kdp", "mi_search_terms"),
        ("zeigarnik_analysis", "Zeigarnik Effect", "Analyzes open loops and unresolved tension to maintain reader engagement.", "craft", ""),
        ("continuity_check", "Continuity Check", "AI-assisted scan for contradicted facts — within a manuscript or across a whole series.", "craft", ""),
        ("show_dont_tell", "Show Don't Tell", "AI-assisted check for telling instead of showing — flags violations with surrounding manuscript text.", "craft", ""),
        ("ai_isms", "AI-isms", "AI-assisted check for prose habits that often read as machine-generated — flags passages with surrounding manuscript text.", "craft", ""),
    ];

    for (id, label, description, platforms, depends_on) in rows {
        conn.execute(
            "INSERT INTO report_types (id, label, description, platforms, depends_on) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET label = excluded.label, description = excluded.description, platforms = excluded.platforms, depends_on = excluded.depends_on",
            params![id, label, description, platforms, depends_on],
        ).map_err(|e| e.to_string())?;
    }

    Ok(())
}

fn seed_prompt_templates(conn: &Connection) -> Result<(), String> {
    // Each row: (id, label, system_prompt, user_template, max_tokens, json_mode)
    let templates: &[(&str, &str, &str, &str, i64, i64)] = &[
        (
            "continuity_extract",
            "Continuity: Fact Extraction",
            r#"You are a continuity editor for fiction. Extract only facts likely to matter for continuity — details a careful reader could later catch a contradiction on if they changed without explanation.

Cover, when present:
- Character: appearance, age, occupation, relationships, skills, habits, possessions
- Place: location, layout, distance from other places
- Object: description, ownership, condition
- Timeline: dates, durations, sequences of events

Use the character or place's fullest, most formal name as it is introduced in the text (e.g. "Sarah Chen" rather than just "Sarah" or "the detective"), so the same entity can be matched consistently across chapters.

Return ONLY a JSON array, no markdown, no preamble. Maximum 15 items. Be terse.
Each item exactly:
{"entity": "<canonical name>", "entity_type": "character|place|object|timeline|other", "attribute": "<short_label>", "value": "<fact in under 10 words>", "snippet": "<verbatim quote, under 12 words>"}
Keep values and snippets as short as possible. One fact per attribute per entity. No duplicates."#,
            "Chapter: {chapter_title}\n\n{bible}\n\n---\n\n{chapter_text}",
            4000, 1
        ),
        (
            "continuity_judge",
            "Continuity: Contradiction Judgment",
            r#"You are a meticulous continuity editor for fiction. For each candidate group below, the same named entity has more than one recorded value for the same attribute across chapters (possibly across different books in a series, in reading order).

Judge whether each is a genuine continuity error a careful reader could catch, or an explainable/intentional change: aging over time, injury, disguise, dye job, unreliable narration, a later-revealed secret, or simply imprecise-but-compatible wording that isn't actually a contradiction (e.g. "green eyes" vs "emerald eyes" is NOT a contradiction).

Return ONLY a JSON array. Each item:
{"entity":"<name>","attribute":"<attr>","verdict":"contradiction|possible|likely_intentional","confidence":<0-100>,"explanation":"<one sentence>"}

Do NOT include reasoning, preamble, or markdown. ONLY the JSON array."#,
            "{bible}\n\n---\n\nCandidate groups:\n{candidates}",
            4000, 1
        ),
        (
            "continuity_suggest",
            "Continuity: Suggest Fix",
            r#"You are a fiction editor helping an author fix a continuity error in their manuscript. You will be given:
- The entity and attribute that has contradicting values
- Where each value appears (which chapter, with a snippet of surrounding text)

Provide 2-3 concrete suggestions for how to fix the inconsistency. For each:
1. State which occurrence(s) to change
2. Give the exact revised prose (ready to paste)
3. One sentence explaining why this fix works

Keep the author's voice and style. Be concise."#,
            "Entity: {entity}\nAttribute: {attribute}\nExplanation: {explanation}\n\n{bible}\n\nOccurrences:\n{occurrences}",
            2000, 0
        ),
        (
            "sdt_check",
            "Show Don't Tell: Check",
            r#"You check fiction for TELLING instead of SHOWING. Output ONLY a JSON array.

A violation is when the author DIRECTLY STATES an emotion, judgment, or conclusion that should be inferred by the reader from action, dialogue, or sensory detail.

Flag ONLY clear violations. Do NOT flag:
- Internal monologue that reveals character voice
- Montage/summary passages that compress time intentionally
- Metaphors, similes, or sensory comparisons
- A character's self-aware observations in close POV
- Stylistic choices that serve pacing or rhythm

For each violation return exactly:
{"telling_text":"<exact quote, max 15 words>","context":"<1-2 surrounding sentences>","why":"<one sentence>","severity":"minor|moderate|major"}

Rules:
- severity "major" = undermines a key emotional beat
- severity "moderate" = weakens the scene noticeably
- severity "minor" = could be tighter but doesn't hurt much
- Maximum 8 per chapter. Only flag what genuinely weakens the prose.
- Return [] if the chapter is clean.
- Do NOT include reasoning, preamble, or markdown. ONLY the JSON array."#,
            "Chapter: {chapter_title}\n\n{bible}\n\n---\n\n{chapter_text}",
            4000, 1
        ),
        (
            "sdt_suggest",
            "Show Don't Tell: Suggest Fix",
            r#"You are a fiction editor helping an author rewrite a "telling" passage to "show" instead. You will be given:
- The passage that tells instead of shows
- Surrounding context
- Why it's considered telling

Provide 2-3 alternative rewrites that SHOW instead of TELL. For each:
1. Give the revised prose (ready to paste — match the author's voice and tense)
2. One sentence explaining the technique used (body language, sensory detail, action, dialogue, etc.)

Keep rewrites concise — replace only the telling passage, not the surrounding context. Maintain the author's style and point of view."#,
            "Chapter: {chapter_title}\n\nTelling passage: \"{telling_text}\"\n\nContext: \"{context}\"\n\nWhy it's telling: {why}\n\n{bible}",
            1500, 0
        ),
        (
            "ai_isms_check",
            "AI-isms: Check",
            r#"You check fiction for AI-isms — prose habits that often sound machine-generated or template-written. Output ONLY a JSON array.

Flag passages that feel synthetic, generic, or LLM-flavored, including:
- Stock AI vocabulary: delve, tapestry, testament to, landscape of, in the realm of, pivotal, underscore, foster, embark, nestled, amidst
- Throat-clearing / essay filler: "It's important to note", "In a world where", "Little did they know"
- Overused antithesis templates: "Not X, but Y" / "It wasn't just X — it was Y"
- Perfectly polished parallel lists or rule-of-three padding that adds no meaning
- Vague atmospheric abstraction instead of concrete sensory detail
- Sudden thesaurus inflation or oddly formal diction that breaks voice
- Em-dash heavy explanatory asides that lecture the reader

Do NOT flag:
- Deliberate stylistic voice that fits the character/narrator
- Genre-appropriate elevated or lyrical prose
- A single unusual word used once with intent
- Normal literary metaphor or sensory writing

For each flag return exactly:
{"telling_text":"<exact quote, max 15 words>","context":"<1-2 surrounding sentences>","why":"<one sentence naming the AI-ism>","severity":"minor|moderate|major"}

Rules:
- severity "major" = loudly synthetic; breaks immersion
- severity "moderate" = noticeably template-like
- severity "minor" = mild habit that could be tighter
- Maximum 8 per chapter. Prefer clear hits over nitpicks.
- Return [] if the chapter is clean.
- Do NOT include reasoning, preamble, or markdown. ONLY the JSON array."#,
            "Chapter: {chapter_title}\n\n{bible}\n\n---\n\n{chapter_text}",
            4000, 1
        ),
        (
            "ai_isms_suggest",
            "AI-isms: Suggest Fix",
            r#"You are a fiction editor helping an author remove AI-sounding prose. You will be given:
- The passage that reads as an AI-ism
- Surrounding context
- Why it was flagged

Provide 2-3 alternative rewrites that sound human and specific. For each:
1. Give the revised prose (ready to paste — match the author's voice and tense)
2. One sentence explaining what you changed (concreteness, voice, cut filler, etc.)

Keep rewrites concise — replace only the flagged passage, not the surrounding context. Maintain POV and style."#,
            "Chapter: {chapter_title}\n\nFlagged passage: \"{telling_text}\"\n\nContext: \"{context}\"\n\nWhy it was flagged: {why}\n\n{bible}",
            1500, 0
        ),
        (
            "chapter_summary",
            "Chapter Summary: Genre Signal Extraction",
            r#"You are a book genre analyst. Read the chapter excerpt and extract genre-relevant signals: themes, tropes, character archetypes, setting type, tone, pacing indicators, and any explicit or implicit genre markers.

Return a SHORT paragraph (3-5 sentences max) summarizing the genre signals. Do not summarize the plot. Focus only on what tells us about the genre and subgenre. Be specific (name tropes, compare to known genres)."#,
            "Chapter: {chapter_title}\n\n{bible}\n\n---\n\n{chapter_text}",
            500, 0
        ),
    ];

    for (id, label, system_prompt, user_template, max_tokens, json_mode) in templates {
        conn.execute(
            "INSERT INTO prompt_templates (id, label, system_prompt, user_template, max_tokens, json_mode, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'))
             ON CONFLICT(id) DO NOTHING",
            params![id, label, system_prompt, user_template, max_tokens, json_mode],
        ).map_err(|e| e.to_string())?;
    }

    Ok(())
}

fn seed_zeigarnik_config_if_empty(conn: &Connection) -> Result<(), String> {
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM zeigarnik_config", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if count > 0 { return Ok(()); }

    let parsed: serde_json::Value = serde_json::from_str(SEED_ZEIGARNIK_CONFIG_JSON)
        .map_err(|e| format!("Cannot parse seed zeigarnik-config.json: {}", e))?;

    let obj = parsed.as_object().ok_or("zeigarnik-config.json must be a JSON object")?;
    for (key, value) in obj {
        if key == "thresholds" {
            // Flatten thresholds into individual keys so each is independently tunable.
            if let Some(t) = value.as_object() {
                for (tkey, tval) in t {
                    conn.execute(
                        "INSERT OR IGNORE INTO zeigarnik_config (key, value) VALUES (?1, ?2)",
                        params![format!("threshold.{}", tkey), tval.to_string()],
                    ).map_err(|e| e.to_string())?;
                }
            }
        } else {
            conn.execute(
                "INSERT OR IGNORE INTO zeigarnik_config (key, value) VALUES (?1, ?2)",
                params![key, value.to_string()],
            ).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

// ── Zeigarnik effect detector (craft platform, no AI) ──────────────────────

#[derive(Clone, Debug)]
pub struct ZeigarnikConfig {
    pub cliffhanger_markers:  Vec<String>,
    pub resolution_markers:   Vec<String>,
    pub question_lead_ins:    Vec<String>,
    pub short_fragment_max_words:      usize,
    pub min_gap_chapters_for_thread:   usize,
    pub max_total_mentions_for_thread: usize,
    pub min_thread_term_len:           usize,
    pub top_threads_limit:             usize,
    pub min_question_words:            usize,
    pub max_questions_per_chapter:     usize,
}

impl Default for ZeigarnikConfig {
    fn default() -> Self {
        ZeigarnikConfig {
            cliffhanger_markers: vec![], resolution_markers: vec![], question_lead_ins: vec![],
            short_fragment_max_words: 8, min_gap_chapters_for_thread: 3,
            max_total_mentions_for_thread: 6, min_thread_term_len: 4,
            top_threads_limit: 25, min_question_words: 4, max_questions_per_chapter: 6,
        }
    }
}

/// Load the Zeigarnik phrase lists and thresholds from the database. Falls
/// back to sane defaults for any key missing (e.g. a fresh DB where seeding
/// somehow failed) rather than erroring the whole analysis out.
pub fn load_zeigarnik_config(conn: &Connection) -> ZeigarnikConfig {
    let mut cfg = ZeigarnikConfig::default();

    let get_str_list = |key: &str| -> Vec<String> {
        conn.query_row("SELECT value FROM zeigarnik_config WHERE key = ?1", params![key], |r| r.get::<_, String>(0))
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
            .unwrap_or_default()
    };
    let get_usize = |key: &str, default: usize| -> usize {
        conn.query_row("SELECT value FROM zeigarnik_config WHERE key = ?1", params![format!("threshold.{}", key)], |r| r.get::<_, String>(0))
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(default)
    };

    cfg.cliffhanger_markers = get_str_list("cliffhanger_markers");
    cfg.resolution_markers  = get_str_list("resolution_markers");
    cfg.question_lead_ins   = get_str_list("question_lead_ins");
    cfg.short_fragment_max_words      = get_usize("short_fragment_max_words", 8);
    cfg.min_gap_chapters_for_thread   = get_usize("min_gap_chapters_for_thread", 3);
    cfg.max_total_mentions_for_thread = get_usize("max_total_mentions_for_thread", 6);
    cfg.min_thread_term_len           = get_usize("min_thread_term_len", 4);
    cfg.top_threads_limit             = get_usize("top_threads_limit", 25);
    cfg.min_question_words            = get_usize("min_question_words", 4);
    cfg.max_questions_per_chapter     = get_usize("max_questions_per_chapter", 6);

    cfg
}

#[derive(Clone, Debug)]
pub struct ZeigarnikChapterRow {
    pub chapter_index:  i64,
    pub file:           String,
    pub title:          String,
    pub word_count:     i64,
    pub sentence_count: i64,
    pub question_count: i64,
    pub ending_type:    String,
    pub tension_score:  i64,
    pub ending_snippet: String,
}

#[derive(Clone, Debug)]
pub struct ZeigarnikThreadRow {
    pub term:                String,
    pub mention_count:       i64,
    pub first_chapter_index: i64,
    pub first_file:          String,
    pub first_snippet:       String,
    pub gap_start_index:     i64,
    pub gap_end_index:       i64,
    pub max_gap_chapters:    i64,
    pub max_gap_words:       i64,
}

/// Replace all stored Zeigarnik chapter metrics + threads for a story with a
/// fresh set — same "latest run supersedes" model used everywhere else.
pub fn replace_zeigarnik_analysis(
    conn: &Connection,
    story_folder: &str,
    chapters: &[ZeigarnikChapterRow],
    threads: &[ZeigarnikThreadRow],
) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute("DELETE FROM zeigarnik_chapters WHERE story_folder = ?1", params![story_folder]).map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM zeigarnik_threads WHERE story_folder = ?1", params![story_folder]).map_err(|e| e.to_string())?;

    for c in chapters {
        conn.execute(
            "INSERT INTO zeigarnik_chapters
             (story_folder, chapter_index, file, title, word_count, sentence_count, question_count, ending_type, tension_score, ending_snippet, generated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![story_folder, c.chapter_index, c.file, c.title, c.word_count, c.sentence_count, c.question_count, c.ending_type, c.tension_score, c.ending_snippet, now],
        ).map_err(|e| e.to_string())?;
    }

    for t in threads {
        conn.execute(
            "INSERT INTO zeigarnik_threads
             (story_folder, term, mention_count, first_chapter_index, first_file, first_snippet, gap_start_index, gap_end_index, max_gap_chapters, max_gap_words, generated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![story_folder, t.term, t.mention_count, t.first_chapter_index, t.first_file, t.first_snippet, t.gap_start_index, t.gap_end_index, t.max_gap_chapters, t.max_gap_words, now],
        ).map_err(|e| e.to_string())?;
    }

    Ok(())
}

pub fn has_zeigarnik_analysis(conn: &Connection, story_folder: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM zeigarnik_chapters WHERE story_folder = ?1 LIMIT 1",
        params![story_folder], |_| Ok(())
    ).is_ok()
}

// ── Series (Continuity Checker: grouping stories in reading order) ────────────

#[derive(serde::Serialize, Clone, Debug)]
pub struct SeriesRow {
    pub id:         i64,
    pub name:       String,
    pub book_count: i64,
}

#[derive(serde::Serialize, Clone, Debug)]
pub struct SeriesBookRow {
    pub story_folder: String,
    pub story_name:   String,
    pub book_order:   i64,
}

pub fn list_series(conn: &Connection) -> Result<Vec<SeriesRow>, String> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.name, COUNT(sb.story_folder)
         FROM series s LEFT JOIN series_books sb ON sb.series_id = s.id
         GROUP BY s.id ORDER BY s.name"
    ).map_err(|e| e.to_string())?;
    let rows = stmt.query_map([], |r| {
        Ok(SeriesRow { id: r.get(0)?, name: r.get(1)?, book_count: r.get(2)? })
    }).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

pub fn create_series(conn: &Connection, name: &str) -> Result<SeriesRow, String> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute("INSERT INTO series (name, created_at) VALUES (?1, ?2)", params![name.trim(), now])
        .map_err(|e| e.to_string())?;
    let id = conn.last_insert_rowid();
    Ok(SeriesRow { id, name: name.trim().to_string(), book_count: 0 })
}

/// Deletes the series and its book memberships. Does NOT delete the stories
/// themselves or any continuity data already recorded under the series key.
pub fn delete_series(conn: &Connection, series_id: i64) -> Result<(), String> {
    conn.execute("DELETE FROM series_books WHERE series_id = ?1", params![series_id]).map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM series WHERE id = ?1", params![series_id]).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn list_series_books(conn: &Connection, series_id: i64) -> Result<Vec<SeriesBookRow>, String> {
    let mut stmt = conn.prepare(
        "SELECT story_folder, story_name, book_order FROM series_books
         WHERE series_id = ?1 ORDER BY book_order"
    ).map_err(|e| e.to_string())?;
    let rows = stmt.query_map(params![series_id], |r| {
        Ok(SeriesBookRow { story_folder: r.get(0)?, story_name: r.get(1)?, book_order: r.get(2)? })
    }).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

pub fn add_story_to_series(conn: &Connection, series_id: i64, story_folder: &str, story_name: &str, book_order: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO series_books (series_id, story_folder, story_name, book_order) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(series_id, story_folder) DO UPDATE SET story_name = excluded.story_name, book_order = excluded.book_order",
        params![series_id, story_folder, story_name, book_order],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn remove_story_from_series(conn: &Connection, series_id: i64, story_folder: &str) -> Result<(), String> {
    conn.execute("DELETE FROM series_books WHERE series_id = ?1 AND story_folder = ?2", params![series_id, story_folder])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn list_series_cmd(db: tauri::State<'_, Db>) -> Result<Vec<SeriesRow>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    list_series(&conn)
}

#[tauri::command]
pub async fn create_series_cmd(db: tauri::State<'_, Db>, name: String) -> Result<SeriesRow, String> {
    if name.trim().is_empty() { return Err("Series name cannot be empty.".to_string()); }
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    create_series(&conn, &name)
}

#[tauri::command]
pub async fn delete_series_cmd(db: tauri::State<'_, Db>, series_id: i64) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    delete_series(&conn, series_id)
}

#[tauri::command]
pub async fn list_series_books_cmd(db: tauri::State<'_, Db>, series_id: i64) -> Result<Vec<SeriesBookRow>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    list_series_books(&conn, series_id)
}

#[derive(serde::Deserialize)]
pub struct AddToSeriesRequest {
    pub series_id:    i64,
    pub story_folder: String,
    pub story_name:   String,
    pub book_order:   i64,
}

#[tauri::command]
pub async fn add_story_to_series_cmd(db: tauri::State<'_, Db>, request: AddToSeriesRequest) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    add_story_to_series(&conn, request.series_id, &request.story_folder, &request.story_name, request.book_order)
}

#[tauri::command]
pub async fn remove_story_from_series_cmd(db: tauri::State<'_, Db>, series_id: i64, story_folder: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    remove_story_from_series(&conn, series_id, &story_folder)
}

// ── Continuity Checker (craft platform, AI-assisted) ────────────────────

#[derive(Clone, Debug)]
pub struct ContinuityFactRow {
    pub chapter_index: i64,
    pub file:          String,
    pub chapter_title: String,
    pub entity:        String,
    pub entity_type:   String,
    pub attribute:     String,
    pub value:         String,
    pub snippet:       String,
}

pub fn replace_continuity_facts(conn: &Connection, story_folder: &str, facts: &[ContinuityFactRow]) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute("DELETE FROM continuity_facts WHERE story_folder = ?1", params![story_folder]).map_err(|e| e.to_string())?;
    for f in facts {
        conn.execute(
            "INSERT INTO continuity_facts
             (story_folder, chapter_index, file, chapter_title, entity, entity_type, attribute, value, snippet, generated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![story_folder, f.chapter_index, f.file, f.chapter_title, f.entity, f.entity_type, f.attribute, f.value, f.snippet, now],
        ).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ContinuityOccurrence {
    pub story_folder:  String,
    pub story_name:    String,
    pub file:          String,
    pub chapter_title: String,
    pub chapter_index: i64,
    pub value:         String,
    pub snippet:       String,
}

#[derive(Clone, Debug)]
pub struct ContinuityFindingRow {
    pub entity:       String,
    pub attribute:    String,
    pub verdict:      String,
    pub confidence:   i64,
    pub explanation:  String,
    pub occurrences:  Vec<ContinuityOccurrence>,
}

/// Replace all stored findings for a scope (one manuscript, or one series) —
/// same "latest run supersedes" model used everywhere else.
pub fn replace_continuity_findings(conn: &Connection, scope: &str, scope_key: &str, findings: &[ContinuityFindingRow]) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute("DELETE FROM continuity_findings WHERE scope = ?1 AND scope_key = ?2", params![scope, scope_key]).map_err(|e| e.to_string())?;
    for f in findings {
        let occ_json = serde_json::to_string(&f.occurrences).unwrap_or_default();
        conn.execute(
            "INSERT INTO continuity_findings
             (scope, scope_key, entity, attribute, verdict, confidence, explanation, occurrences_json, generated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![scope, scope_key, f.entity, f.attribute, f.verdict, f.confidence, f.explanation, occ_json, now],
        ).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ── Tauri commands (for future UI — browsing/editing the genre/category map) ───────────

#[tauri::command]
pub async fn list_genres_cmd(db: tauri::State<'_, Db>) -> Result<Vec<GenreRow>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    list_genres(&conn)
}

#[derive(serde::Deserialize)]
pub struct AddKdpPathRequest {
    pub genre_name: String,
    pub path:       String,
    pub store:      String,
}

#[tauri::command]
pub async fn add_kdp_path_cmd(db: tauri::State<'_, Db>, request: AddKdpPathRequest) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    upsert_kdp_path(&conn, &request.genre_name, &request.path, &request.store, "manual", false)
}

// ── Query helpers used by genre_analyzer.rs / category_finder.rs ──────────────

#[derive(serde::Serialize, Clone, Debug)]
pub struct GenreRow {
    pub id:          i64,
    pub name:        String,
    pub description: String,
}

pub fn list_genres(conn: &Connection) -> Result<Vec<GenreRow>, String> {
    let mut stmt = conn.prepare("SELECT id, name, description FROM genres ORDER BY name")
        .map_err(|e| e.to_string())?;
    let rows = stmt.query_map([], |r| {
        Ok(GenreRow { id: r.get(0)?, name: r.get(1)?, description: r.get(2)? })
    }).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// Get every known KDP path for a genre name (by exact name match).
pub fn kdp_paths_for_genre(conn: &Connection, genre_name: &str, store: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn.prepare(
        "SELECT kc.path FROM kdp_categories kc
         JOIN genre_kdp_links gkl ON gkl.category_id = kc.id
         JOIN genres g ON g.id = gkl.genre_id
         WHERE g.name = ?1 AND kc.store = ?2
         ORDER BY kc.verified_at DESC NULLS LAST"
    ).map_err(|e| e.to_string())?;
    let rows = stmt.query_map(params![genre_name, store], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// Record (or update) a KDP category path and link it to a genre. Used both
/// for manual corrections and for auto-growth from Category Finder results.
/// Marks the path as verified (sets verified_at) when `verified` is true —
/// i.e. when it came from a live, successful category lookup.
pub fn upsert_kdp_path(
    conn: &Connection,
    genre_name: &str,
    path: &str,
    store: &str,
    source: &str,
    verified: bool,
) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO kdp_categories (path, store, source, verified_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(path, store) DO UPDATE SET
            verified_at = CASE WHEN ?4 IS NOT NULL THEN ?4 ELSE kdp_categories.verified_at END,
            source = excluded.source",
        params![path, store, source, if verified { Some(now.clone()) } else { None::<String> }, now],
    ).map_err(|e| e.to_string())?;

    let category_id: i64 = conn.query_row(
        "SELECT id FROM kdp_categories WHERE path = ?1 AND store = ?2",
        params![path, store], |r| r.get(0)
    ).map_err(|e| e.to_string())?;

    conn.execute(
        "INSERT OR IGNORE INTO genres (name, description) VALUES (?1, '')",
        params![genre_name],
    ).map_err(|e| e.to_string())?;

    let genre_id: i64 = conn.query_row(
        "SELECT id FROM genres WHERE name = ?1", params![genre_name], |r| r.get(0)
    ).map_err(|e| e.to_string())?;

    conn.execute(
        "INSERT OR IGNORE INTO genre_kdp_links (genre_id, category_id) VALUES (?1, ?2)",
        params![genre_id, category_id],
    ).map_err(|e| e.to_string())?;

    Ok(())
}

/// Replace all stored genre rankings for a story with a fresh set — "latest
/// ranking wins" rather than accumulating history, since re-running Rank
/// Genres means the previous ranking is superseded, not a separate data point.
pub fn replace_genre_rankings(
    conn: &Connection,
    story_folder: &str,
    rankings: &[(String, u8, String)],  // (genre_name, confidence, reason)
) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute("DELETE FROM genre_rankings WHERE story_folder = ?1", params![story_folder])
        .map_err(|e| e.to_string())?;

    for (genre_name, confidence, reason) in rankings {
        conn.execute(
            "INSERT OR IGNORE INTO genres (name, description) VALUES (?1, '')",
            params![genre_name],
        ).map_err(|e| e.to_string())?;
        let genre_id: i64 = conn.query_row(
            "SELECT id FROM genres WHERE name = ?1", params![genre_name], |r| r.get(0)
        ).map_err(|e| e.to_string())?;

        conn.execute(
            "INSERT INTO genre_rankings (story_folder, genre_id, confidence, reason, generated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![story_folder, genre_id, confidence, reason, now],
        ).map_err(|e| e.to_string())?;
    }

    Ok(())
}

#[derive(serde::Serialize, Clone, Debug)]
pub struct RankingRow {
    pub genre:      String,
    pub confidence: i64,
    pub reason:     String,
    pub kdp_paths:  Vec<String>,
}

pub fn get_genre_rankings(conn: &Connection, story_folder: &str, store: &str) -> Result<Vec<RankingRow>, String> {
    let mut stmt = conn.prepare(
        "SELECT g.name, gr.confidence, gr.reason
         FROM genre_rankings gr JOIN genres g ON g.id = gr.genre_id
         WHERE gr.story_folder = ?1
         ORDER BY gr.confidence DESC"
    ).map_err(|e| e.to_string())?;

    let rows: Vec<(String, i64, String)> = stmt.query_map(params![story_folder], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
    }).map_err(|e| e.to_string())?
      .collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for (genre, confidence, reason) in rows {
        let kdp_paths = kdp_paths_for_genre(conn, &genre, store)?;
        out.push(RankingRow { genre, confidence, reason, kdp_paths });
    }
    Ok(out)
}

pub fn has_genre_rankings(conn: &Connection, story_folder: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM genre_rankings WHERE story_folder = ?1 LIMIT 1",
        params![story_folder], |_| Ok(())
    ).is_ok()
}

pub fn has_category_results(conn: &Connection, story_folder: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM category_results WHERE story_folder = ?1 LIMIT 1",
        params![story_folder], |_| Ok(())
    ).is_ok()
}

pub fn kdp_category_count(conn: &Connection, store: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM kdp_categories WHERE store = ?1",
        params![store], |r| r.get(0)
    ).unwrap_or(0)
}

/// Keyword search over the imported category catalog — case-insensitive
/// substring match per term, deduplicated, capped at `limit`. This is the
/// direct replacement for Category Finder's live top-level scraping: once
/// the catalog is populated (WinningCat import, or prior discoveries), this
/// is a plain SQL query instead of scraping any external UI at all.
pub fn search_kdp_categories(conn: &Connection, store: &str, terms: &[String], limit: usize) -> Vec<(String, String)> {
    if terms.is_empty() { return Vec::new(); }
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();

    for term in terms {
        let cleaned = term.replace('%', " ").replace('_', " ");
        let cleaned = cleaned.trim();
        if cleaned.is_empty() { continue; }
        let pattern = format!("%{}%", cleaned);

        let mut stmt = match conn.prepare(
            "SELECT path, COALESCE(amazon_node_id,'') FROM kdp_categories
             WHERE store = ?1 AND path LIKE ?2 ESCAPE '\\' COLLATE NOCASE LIMIT 200"
        ) { Ok(s) => s, Err(_) => continue };

        let rows: Vec<(String, String)> = match stmt.query_map(params![store, pattern], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        }) {
            Ok(mapped) => mapped.flatten().collect(),
            Err(_) => continue,
        };

        for row in rows {
            if seen.insert(row.0.clone()) {
                out.push(row);
                if out.len() >= limit { return out; }
            }
        }
    }
    out
}

/// Import a category path + node ID from an external catalog (WinningCat)
/// without linking it to any genre yet — that happens later via Category
/// Finder discovery or manual mapping. Preserves the source label if a path
/// was already verified live (category_finder /
/// category_analyzer outrank a catalog import), but always refreshes the
/// node ID and last_seen_at since those are authoritative either way.
pub fn import_kdp_category(conn: &Connection, path: &str, store: &str, node_id: &str) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO kdp_categories (path, store, amazon_node_id, source, created_at, last_seen_at)
         VALUES (?1, ?2, ?3, 'winningcat', ?4, ?4)
         ON CONFLICT(path, store) DO UPDATE SET
            amazon_node_id = excluded.amazon_node_id,
            last_seen_at = ?4,
            source = CASE
                WHEN kdp_categories.source IN ('category_finder', 'category_analyzer')
                THEN kdp_categories.source ELSE 'winningcat' END",
        params![path, store, node_id, now],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

/// Every catalog entry sourced from WinningCat that was NOT touched by an
/// import run started at or after `since` — i.e. it was in a previous
/// WinningCat file but missing from the latest one. Doesn't delete anything
/// automatically (Amazon renaming a category and it genuinely disappearing
/// look identical from here); surfaces the list so a human decides.
pub fn stale_winningcat_paths(conn: &Connection, since: &str) -> Vec<(String, String)> {
    let mut stmt = match conn.prepare(
        "SELECT path, store FROM kdp_categories
         WHERE source = 'winningcat' AND (last_seen_at IS NULL OR last_seen_at < ?1)
         ORDER BY store, path"
    ) { Ok(s) => s, Err(_) => return Vec::new() };
    stmt.query_map(params![since], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
        .unwrap_or_default()
}

/// Remove every WinningCat-sourced catalog entry not seen since `since`.
/// Called only when the user explicitly confirms cleanup after reviewing
/// the stale count from an import — not automatic.
pub fn remove_stale_winningcat_paths(conn: &Connection, since: &str) -> Result<usize, String> {
    conn.execute(
        "DELETE FROM kdp_categories WHERE source = 'winningcat' AND (last_seen_at IS NULL OR last_seen_at < ?1)",
        params![since],
    ).map_err(|e| e.to_string())
}

/// Replace all stored category-finder results for a story with a fresh set.
/// Every matched/considered result also gets written into kdp_categories and
/// linked to the genre it was found under (when it clears 80%, marked
/// verified — this is how the genre->KDP map grows from real usage).
pub fn replace_category_results(
    conn: &Connection,
    story_folder: &str,
    store: &str,
    top_genre_hint: Option<&str>,
    results: &[(String, u8, String, String, String, String, String, Option<String>)],
    // (path, confidence, sales_to_one, sales_to_ten, publisher_pct, ku_pct, status, note)
) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute("DELETE FROM category_results WHERE story_folder = ?1", params![story_folder])
        .map_err(|e| e.to_string())?;

    for (path, confidence, sales_to_one, sales_to_ten, publisher_pct, ku_pct, status, note) in results {
        let category_id: Option<i64> = if status != "failed" {
            let _ = conn.execute(
                "INSERT INTO kdp_categories (path, store, source, verified_at, created_at)
                 VALUES (?1, ?2, 'category_finder', ?3, ?3)
                 ON CONFLICT(path, store) DO UPDATE SET verified_at = ?3, source = 'category_finder'",
                params![path, store, now],
            );
            let id: Option<i64> = conn.query_row(
                "SELECT id FROM kdp_categories WHERE path = ?1 AND store = ?2",
                params![path, store], |r| r.get(0)
            ).ok();

            if let (Some(cat_id), Some(genre_name)) = (id, top_genre_hint) {
                let _ = conn.execute(
                    "INSERT OR IGNORE INTO genres (name, description) VALUES (?1, '')",
                    params![genre_name],
                );
                if let Ok(genre_id) = conn.query_row::<i64, _, _>(
                    "SELECT id FROM genres WHERE name = ?1", params![genre_name], |r| r.get(0)
                ) {
                    let _ = conn.execute(
                        "INSERT OR IGNORE INTO genre_kdp_links (genre_id, category_id) VALUES (?1, ?2)",
                        params![genre_id, cat_id],
                    );
                }
            }
            id
        } else {
            None
        };

        conn.execute(
            "INSERT INTO category_results
             (story_folder, category_id, raw_path, store, confidence, sales_to_one, sales_to_ten,
              publisher_pct, ku_pct, status, note, generated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![story_folder, category_id, path, store, confidence, sales_to_one, sales_to_ten,
                     publisher_pct, ku_pct, status, note, now],
        ).map_err(|e| e.to_string())?;
    }

    Ok(())
}

// ── Chapter summaries ──────────────────────────────────────────────────

#[derive(serde::Serialize, Clone, Debug)]
pub struct ChapterSummaryRow {
    pub file:       String,
    pub title:      String,
    pub signals:    String,
    pub word_count: i64,
}

pub fn chapter_summary_exists(conn: &Connection, story_folder: &str, file: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM chapter_summaries WHERE story_folder = ?1 AND file = ?2",
        params![story_folder, file], |_| Ok(())
    ).is_ok()
}

pub fn save_chapter_summary(
    conn: &Connection, story_folder: &str, file: &str, title: &str, signals: &str, word_count: i64,
) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO chapter_summaries (story_folder, file, title, signals, word_count, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(story_folder, file) DO UPDATE SET
            title = excluded.title, signals = excluded.signals,
            word_count = excluded.word_count, updated_at = excluded.updated_at",
        params![story_folder, file, title, signals, word_count, now],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn load_chapter_summaries(conn: &Connection, story_folder: &str) -> Vec<ChapterSummaryRow> {
    let mut stmt = match conn.prepare(
        "SELECT file, title, signals, word_count FROM chapter_summaries
         WHERE story_folder = ?1 ORDER BY file"
    ) { Ok(s) => s, Err(_) => return Vec::new() };

    stmt.query_map(params![story_folder], |r| {
        Ok(ChapterSummaryRow {
            file: r.get(0)?, title: r.get(1)?, signals: r.get(2)?, word_count: r.get(3)?,
        })
    }).and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
       .unwrap_or_default()
}

pub fn chapter_summary_count(conn: &Connection, story_folder: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM chapter_summaries WHERE story_folder = ?1",
        params![story_folder], |r| r.get(0)
    ).unwrap_or(0)
}

/// Wipe all chapter summaries for a story so the next Analyze run
/// regenerates every chapter from scratch, instead of skipping ones that
/// already have a summary. Used by the "force re-summarize" checkbox.
pub fn delete_chapter_summaries(conn: &Connection, story_folder: &str) -> Result<(), String> {
    conn.execute("DELETE FROM chapter_summaries WHERE story_folder = ?1", params![story_folder])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ── Genre classification (industry genre + KDP paths + comps + notes) ──────────────

#[derive(Clone, Debug)]

pub struct GenreDataRow {
    pub industry_ebook:     String,
    pub industry_print:     String,
    pub genre_signals:      String,
    pub reader_demographic: String,
    pub bookstore_shelving: String,
    pub kdp_ebook:          Vec<String>,
    pub kdp_print:          Vec<String>,
    pub comps_ebook:        Vec<String>,
    pub comps_print:        Vec<String>,
    pub marketing_notes:    Vec<String>,
}

#[allow(clippy::too_many_arguments)]
pub fn save_genre_data(
    conn: &Connection,
    story_folder: &str,
    industry_ebook: &str,
    industry_print: &str,
    genre_signals: &str,
    reader_demographic: &str,
    bookstore_shelving: &str,
    kdp_ebook: &[String],
    kdp_print: &[String],
    comps_ebook: &[String],
    comps_print: &[String],
    marketing_notes: &[String],
) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO genre_data
         (story_folder, generated_at, industry_ebook, industry_print, genre_signals,
          reader_demographic, bookstore_shelving, kdp_ebook_json, kdp_print_json,
          comps_ebook_json, comps_print_json, marketing_notes_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(story_folder) DO UPDATE SET
            generated_at = excluded.generated_at,
            industry_ebook = excluded.industry_ebook,
            industry_print = excluded.industry_print,
            genre_signals = excluded.genre_signals,
            reader_demographic = excluded.reader_demographic,
            bookstore_shelving = excluded.bookstore_shelving,
            kdp_ebook_json = excluded.kdp_ebook_json,
            kdp_print_json = excluded.kdp_print_json,
            comps_ebook_json = excluded.comps_ebook_json,
            comps_print_json = excluded.comps_print_json,
            marketing_notes_json = excluded.marketing_notes_json",
        params![
            story_folder, now, industry_ebook, industry_print, genre_signals,
            reader_demographic, bookstore_shelving,
            serde_json::to_string(kdp_ebook).unwrap_or_default(),
            serde_json::to_string(kdp_print).unwrap_or_default(),
            serde_json::to_string(comps_ebook).unwrap_or_default(),
            serde_json::to_string(comps_print).unwrap_or_default(),
            serde_json::to_string(marketing_notes).unwrap_or_default(),
        ],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn load_genre_data(conn: &Connection, story_folder: &str) -> Option<GenreDataRow> {
    conn.query_row(
        "SELECT industry_ebook, industry_print, genre_signals, reader_demographic,
                bookstore_shelving, kdp_ebook_json, kdp_print_json, comps_ebook_json,
                comps_print_json, marketing_notes_json
         FROM genre_data WHERE story_folder = ?1",
        params![story_folder],
        |r| {
            let parse = |s: String| serde_json::from_str::<Vec<String>>(&s).unwrap_or_default();
            Ok(GenreDataRow {
                industry_ebook:     r.get(0)?,
                industry_print:     r.get(1)?,
                genre_signals:      r.get(2)?,
                reader_demographic: r.get(3)?,
                bookstore_shelving: r.get(4)?,
                kdp_ebook:          parse(r.get(5)?),
                kdp_print:          parse(r.get(6)?),
                comps_ebook:        parse(r.get(7)?),
                comps_print:        parse(r.get(8)?),
                marketing_notes:    parse(r.get(9)?),
            })
        },
    ).ok()
}

// ── KDP keywords (the 7 ready-to-paste strings) ──────────────────────────

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct KdpKeywordEntry {
    pub string:    String,
    pub chars:     i64,
    pub rationale: String,
}

pub fn save_kdp_keywords(
    conn: &Connection, story_folder: &str, keywords: &[KdpKeywordEntry], strategy: &str, source_note: &str,
) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO kdp_keywords (story_folder, generated_at, keywords_json, strategy, source_note)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(story_folder) DO UPDATE SET
            generated_at = excluded.generated_at, keywords_json = excluded.keywords_json,
            strategy = excluded.strategy, source_note = excluded.source_note",
        params![story_folder, now, serde_json::to_string(keywords).unwrap_or_default(), strategy, source_note],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn load_kdp_keywords(conn: &Connection, story_folder: &str) -> Option<(Vec<KdpKeywordEntry>, String, String)> {
    conn.query_row(
        "SELECT keywords_json, strategy, source_note FROM kdp_keywords WHERE story_folder = ?1",
        params![story_folder],
        |r| {
            let json: String = r.get(0)?;
            let strategy: Option<String> = r.get(1)?;
            let note: Option<String> = r.get(2)?;
            Ok((json, strategy.unwrap_or_default(), note.unwrap_or_default()))
        },
    ).ok().map(|(json, strategy, note)| {
        let keywords: Vec<KdpKeywordEntry> = serde_json::from_str(&json).unwrap_or_default();
        (keywords, strategy, note)
    })
}

// ── MI search-term keywords ─────────────────────────────────────────

pub fn save_mi_search_terms(conn: &Connection, story_folder: &str, keywords: &[String]) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO mi_search_terms (story_folder, generated_at, keywords_json)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(story_folder) DO UPDATE SET generated_at = excluded.generated_at, keywords_json = excluded.keywords_json",
        params![story_folder, now, serde_json::to_string(keywords).unwrap_or_default()],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn load_mi_search_terms(conn: &Connection, story_folder: &str) -> Vec<String> {
    conn.query_row(
        "SELECT keywords_json FROM mi_search_terms WHERE story_folder = ?1",
        params![story_folder], |r| r.get::<_, String>(0)
    ).ok()
     .and_then(|json| serde_json::from_str(&json).ok())
     .unwrap_or_default()
}

// ── Non-KDP discovery keywords (broader platforms: Apple Books, Kobo, etc.) ──

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct DiscoveryKeywordEntry {
    pub phrase:    String,
    pub rationale: String,
}

pub fn save_discovery_keywords(conn: &Connection, story_folder: &str, entries: &[DiscoveryKeywordEntry]) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO discovery_keywords (story_folder, generated_at, keywords_json)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(story_folder) DO UPDATE SET generated_at = excluded.generated_at, keywords_json = excluded.keywords_json",
        params![story_folder, now, serde_json::to_string(entries).unwrap_or_default()],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn load_discovery_keywords(conn: &Connection, story_folder: &str) -> Vec<DiscoveryKeywordEntry> {
    conn.query_row(
        "SELECT keywords_json FROM discovery_keywords WHERE story_folder = ?1",
        params![story_folder], |r| r.get::<_, String>(0)
    ).ok()
     .and_then(|json| serde_json::from_str(&json).ok())
     .unwrap_or_default()
}

pub fn has_keyword_search_results(conn: &Connection, story_folder: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM keyword_search_results WHERE story_folder = ?1)",
        params![story_folder],
        |r| r.get::<_, bool>(0),
    ).unwrap_or(false)
}

// ── Keyword Search results (real search volume / competition) ──

/// Replace all stored results for this story+seed — latest search wins, same
/// "supersede, don't accumulate" model used everywhere else in this app.
pub fn replace_keyword_search_results(
    conn: &Connection, story_folder: &str, seed: &str,
    rows: &[(String, String, String, String)],  // (keyword, searches, competition, earnings)
) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "DELETE FROM keyword_search_results WHERE story_folder = ?1 AND seed = ?2",
        params![story_folder, seed],
    ).map_err(|e| e.to_string())?;
    for (keyword, searches, competition, earnings) in rows {
        conn.execute(
            "INSERT INTO keyword_search_results (story_folder, seed, keyword, searches, competition, earnings, generated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![story_folder, seed, keyword, searches, competition, earnings, now],
        ).map_err(|e| e.to_string())?;
    }
    Ok(())
}


// ── Story documents (rendered markdown cache, read by the Reports panel) ─────

/// Look up the display label for a doc_type from the report_types table.
/// Falls back to the doc_type string itself if not found.
fn label_for_doc_type(conn: &Connection, doc_type: &str) -> String {
    conn.query_row(
        "SELECT label FROM report_types WHERE id = ?1",
        params![doc_type],
        |r| r.get::<_, String>(0),
    ).unwrap_or_else(|_| doc_type.to_string())
}

pub fn save_document(conn: &Connection, story_folder: &str, doc_type: &str, content: &str) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    save_document_at(conn, story_folder, doc_type, content, &now)
}

pub fn save_document_at(conn: &Connection, story_folder: &str, doc_type: &str, content: &str, timestamp: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO story_documents (story_folder, doc_type, content, generated_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![story_folder, doc_type, content, timestamp],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn get_document(conn: &Connection, story_folder: &str, doc_type: &str) -> Option<String> {
    conn.query_row(
        "SELECT content FROM story_documents WHERE story_folder = ?1 AND doc_type = ?2 ORDER BY generated_at DESC LIMIT 1",
        params![story_folder, doc_type], |r| r.get(0)
    ).ok()
}

#[derive(serde::Serialize, Clone, Debug)]
pub struct DocMeta {
    pub id:           i64,
    pub doc_type:     String,
    pub label:        String,
    pub generated_at: String,
}

/// Response envelope for get_report_cmd — tells the frontend what format to expect.
#[derive(serde::Serialize, Clone, Debug)]
pub struct ReportEnvelope {
    pub id:           i64,
    pub doc_type:     String,
    pub label:        String,
    pub format:       String,   // "json" | "markdown"
    pub content:      String,
    pub generated_at: String,
}

pub fn list_documents(conn: &Connection, story_folder: &str) -> Vec<DocMeta> {
    let mut stmt = match conn.prepare(
        "SELECT id, doc_type, generated_at FROM story_documents WHERE story_folder = ?1 ORDER BY generated_at DESC"
    ) { Ok(s) => s, Err(_) => return Vec::new() };

    let rows: Vec<(i64, String, String)> = stmt.query_map(params![story_folder], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
    }).and_then(|rows| rows.collect::<Result<Vec<_>, _>>()).unwrap_or_default();

    rows.into_iter().map(|(id, doc_type, generated_at)| {
        let label = label_for_doc_type(conn, &doc_type);
        DocMeta { id, doc_type, label, generated_at }
    }).collect()
}

// ── Tauri commands for the Reports panel ────────────────────────────

#[derive(serde::Serialize, Clone, Debug)]
pub struct ReportTypeDef {
    pub id:          String,
    pub label:       String,
    pub description: String,
    pub platforms:   Vec<String>,
    pub depends_on:  Vec<String>,
}

#[tauri::command]
pub async fn list_report_types_cmd(db: tauri::State<'_, Db>) -> Result<Vec<ReportTypeDef>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn.prepare(
        "SELECT id, label, description, platforms, depends_on FROM report_types ORDER BY rowid"
    ).map_err(|e| e.to_string())?;

    let rows = stmt.query_map([], |r| {
        Ok(ReportTypeDef {
            id:          r.get(0)?,
            label:       r.get(1)?,
            description: r.get(2)?,
            platforms:   r.get::<_, String>(3)?.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
            depends_on:  r.get::<_, String>(4)?.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        })
    }).map_err(|e| e.to_string())?;

    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_reports_cmd(db: tauri::State<'_, Db>, folder: String) -> Result<Vec<DocMeta>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    Ok(list_documents(&conn, &folder))
}

#[tauri::command]
pub async fn save_activity_log_cmd(db: tauri::State<'_, Db>, folder: String, content: String, timestamp: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let ts = if timestamp.is_empty() { chrono::Utc::now().to_rfc3339() } else { timestamp };
    save_document_at(&conn, &folder, "activity_log", &content, &ts)
}

#[tauri::command]
pub async fn get_report_cmd(db: tauri::State<'_, Db>, id: i64) -> Result<ReportEnvelope, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let (doc_type, content, generated_at): (String, String, String) = conn.query_row(
        "SELECT doc_type, content, generated_at FROM story_documents WHERE id = ?1",
        params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))
    ).map_err(|_| "Report not found.".to_string())?;

    let label = label_for_doc_type(&conn, &doc_type);

    let format = if content.starts_with('{') || content.starts_with('[') {
        if serde_json::from_str::<serde_json::Value>(&content).is_ok() { "json" } else { "markdown" }
    } else {
        "markdown"
    };

    Ok(ReportEnvelope { id, doc_type, label, format: format.to_string(), content, generated_at })
}

// ── Delete a report version ────────────────────────

#[tauri::command]
pub async fn delete_report_cmd(db: tauri::State<'_, Db>, id: i64) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let affected = conn.execute("DELETE FROM story_documents WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    if affected == 0 { return Err("Report not found.".to_string()); }
    Ok(())
}

// ── Sidebar data (grouped reports by platform) ─────────────────────────

#[derive(serde::Serialize, Clone, Debug)]
pub struct SidebarReportVersion {
    pub id:           i64,
    pub generated_at: String,
}

#[derive(serde::Serialize, Clone, Debug)]
pub struct SidebarReportGroup {
    pub doc_type:    String,
    pub label:       String,
    pub description: String,
    pub count:       usize,
    pub versions:    Vec<SidebarReportVersion>,
}

/// Returns reports grouped by type, filtered by platform, sorted newest-first.
/// This is the single source of truth for the sidebar's report list.
#[tauri::command]
pub async fn get_sidebar_reports(db: tauri::State<'_, Db>, folder: String, platform: String) -> Result<Vec<SidebarReportGroup>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    // Get report types for this platform
    let mut type_stmt = conn.prepare(
        "SELECT id, label, description FROM report_types ORDER BY rowid"
    ).map_err(|e| e.to_string())?;
    let all_types: Vec<(String, String, String)> = type_stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
    }).map_err(|e| e.to_string())?
      .filter_map(|r| r.ok())
      .collect();

    // Get platforms for each type
    let mut plat_stmt = conn.prepare(
        "SELECT id, platforms FROM report_types"
    ).map_err(|e| e.to_string())?;
    let plat_map: std::collections::HashMap<String, Vec<String>> = plat_stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    }).map_err(|e| e.to_string())?
      .filter_map(|r| r.ok())
      .map(|(id, platforms)| {
          let plats: Vec<String> = platforms.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
          (id, plats)
      })
      .collect();

    // Get all saved documents for this folder
    let docs = list_documents(&conn, &folder);

    // Group docs by doc_type, sorted newest first (already sorted by query)
    let mut versions_by_type: std::collections::HashMap<String, Vec<SidebarReportVersion>> = std::collections::HashMap::new();
    for doc in &docs {
        versions_by_type.entry(doc.doc_type.clone()).or_default().push(SidebarReportVersion {
            id: doc.id,
            generated_at: doc.generated_at.clone(),
        });
    }

    // Build result: only types that belong to the requested platform
    let groups: Vec<SidebarReportGroup> = all_types.into_iter()
        .filter(|(id, _, _)| {
            plat_map.get(id).map(|p| p.contains(&platform)).unwrap_or(false)
        })
        .map(|(id, label, description)| {
            let versions = versions_by_type.remove(&id).unwrap_or_default();
            let count = versions.len();
            SidebarReportGroup { doc_type: id, label, description, count, versions }
        })
        .collect();

    Ok(groups)
}

// ── BISAC classifications ──────────────────────────────────────────────

#[derive(serde::Serialize, Clone, Debug)]
pub struct BisacCodeRow {
    pub code:    String,
    pub heading: String,
}

pub fn master_bisac_list(conn: &Connection) -> Vec<BisacCodeRow> {
    let mut stmt = match conn.prepare("SELECT code, heading FROM bisac_codes ORDER BY code")
        { Ok(s) => s, Err(_) => return Vec::new() };
    stmt.query_map([], |r| Ok(BisacCodeRow { code: r.get(0)?, heading: r.get(1)? }))
        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
        .unwrap_or_default()
}

/// Replace all stored BISAC classifications for a story+format — latest call
/// wins, same "supersede, don't accumulate" model as genre rankings. `format`
/// is "ebook" or "print", scored and stored independently since a print-only
/// distribution can legitimately warrant different codes than the ebook.
pub fn replace_bisac_classifications(
    conn: &Connection, story_folder: &str, format: &str, rows: &[(String, String, u8, String)],
    // (code, heading, confidence, reason)
) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute("DELETE FROM bisac_classifications WHERE story_folder = ?1 AND format = ?2", params![story_folder, format])
        .map_err(|e| e.to_string())?;
    for (code, heading, confidence, reason) in rows {
        conn.execute(
            "INSERT INTO bisac_classifications (story_folder, code, heading, confidence, reason, generated_at, format)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![story_folder, code, heading, confidence, reason, now, format],
        ).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn has_bisac_classifications(conn: &Connection, story_folder: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM bisac_classifications WHERE story_folder = ?1 LIMIT 1",
        params![story_folder], |_| Ok(())
    ).is_ok()
}

// ── Top-level KDP categories (derived from catalog) ─────────────────────

/// Extract the distinct top-level segments from all kdp_categories paths for
/// Look up the Amazon node ID for a category path. Returns None if the path
/// isn't in the catalog or has no node ID (manually-added paths without WinningCat data).
pub fn node_id_for_path(conn: &Connection, path: &str, store: &str) -> Option<String> {
    conn.query_row(
        "SELECT amazon_node_id FROM kdp_categories WHERE path = ?1 AND store = ?2 AND amazon_node_id IS NOT NULL AND amazon_node_id != ''",
        params![path, store], |r| r.get::<_, String>(0)
    ).ok()
}
