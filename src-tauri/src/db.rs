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
    verified_at    TEXT,                              -- last time confirmed live in Publisher Rocket
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
    keywords      TEXT,
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

CREATE TABLE IF NOT EXISTS pr_keywords (
    story_folder  TEXT PRIMARY KEY,
    generated_at  TEXT NOT NULL,
    keywords_json TEXT NOT NULL   -- ["kw1","kw2",...]
);

-- Rendered markdown cache for the Reports panel. Every report type is
-- re-rendered fresh from its structured source table whenever regenerated —
-- this table is what the UI reads, never hand-edited, never stale-checked
-- against a file that quietly stopped being written (see: the genre-ranking
-- .json/.md drift this replaces).
CREATE TABLE IF NOT EXISTS story_documents (
    story_folder TEXT NOT NULL,
    doc_type     TEXT NOT NULL,
    content      TEXT NOT NULL,
    generated_at TEXT NOT NULL,
    PRIMARY KEY (story_folder, doc_type)
);

CREATE INDEX IF NOT EXISTS idx_summaries_folder ON chapter_summaries(story_folder);
"#;

pub struct Db(pub Mutex<Connection>);

/// Open (or create) the app's SQLite database in the platform app-data
/// directory, apply schema, and seed from JSON on first run only.
pub fn init(app: &AppHandle) -> Result<Db, String> {
    let dir: PathBuf = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create app data dir: {}", e))?;

    let db_path = dir.join("pub-rocket-reader.db");
    let conn = Connection::open(&db_path).map_err(|e| format!("Cannot open database: {}", e))?;
    conn.execute_batch(SCHEMA).map_err(|e| format!("Schema error: {}", e))?;

    seed_if_empty(&conn)?;

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
/// i.e. when it came from a live, successful Publisher Rocket lookup.
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

/// Replace all stored category-finder results for a story with a fresh set.
/// Every matched/considered result also gets written into kdp_categories and
/// linked to the genre it was found under (when it clears 80%, marked
/// verified — this is how the genre->KDP map grows from real usage).
pub fn replace_category_results(
    conn: &Connection,
    story_folder: &str,
    store: &str,
    top_genre_hint: Option<&str>,
    results: &[(String, u8, String, String, String, String, String, String, Option<String>)],
    // (path, confidence, sales_to_one, sales_to_ten, publisher_pct, ku_pct, keywords, status, note)
) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute("DELETE FROM category_results WHERE story_folder = ?1", params![story_folder])
        .map_err(|e| e.to_string())?;

    for (path, confidence, sales_to_one, sales_to_ten, publisher_pct, ku_pct, keywords, status, note) in results {
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
              publisher_pct, ku_pct, keywords, status, note, generated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![story_folder, category_id, path, store, confidence, sales_to_one, sales_to_ten,
                     publisher_pct, ku_pct, keywords, status, note, now],
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

// ── Genre classification (industry genre + KDP paths + comps + notes) ──────────────

#[derive(Clone, Debug)]
pub struct GenreDataRow {
    pub generated_at:       String,
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
        "SELECT generated_at, industry_ebook, industry_print, genre_signals, reader_demographic,
                bookstore_shelving, kdp_ebook_json, kdp_print_json, comps_ebook_json,
                comps_print_json, marketing_notes_json
         FROM genre_data WHERE story_folder = ?1",
        params![story_folder],
        |r| {
            let parse = |s: String| serde_json::from_str::<Vec<String>>(&s).unwrap_or_default();
            Ok(GenreDataRow {
                generated_at:       r.get(0)?,
                industry_ebook:     r.get(1)?,
                industry_print:     r.get(2)?,
                genre_signals:      r.get(3)?,
                reader_demographic: r.get(4)?,
                bookstore_shelving: r.get(5)?,
                kdp_ebook:          parse(r.get(6)?),
                kdp_print:          parse(r.get(7)?),
                comps_ebook:        parse(r.get(8)?),
                comps_print:        parse(r.get(9)?),
                marketing_notes:    parse(r.get(10)?),
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

// ── PR search-term keywords ─────────────────────────────────────────

pub fn save_pr_keywords(conn: &Connection, story_folder: &str, keywords: &[String]) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO pr_keywords (story_folder, generated_at, keywords_json)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(story_folder) DO UPDATE SET generated_at = excluded.generated_at, keywords_json = excluded.keywords_json",
        params![story_folder, now, serde_json::to_string(keywords).unwrap_or_default()],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn load_pr_keywords(conn: &Connection, story_folder: &str) -> Vec<String> {
    conn.query_row(
        "SELECT keywords_json FROM pr_keywords WHERE story_folder = ?1",
        params![story_folder], |r| r.get::<_, String>(0)
    ).ok()
     .and_then(|json| serde_json::from_str(&json).ok())
     .unwrap_or_default()
}

// ── Story documents (rendered markdown cache, read by the Reports panel) ─────

pub const DOC_TYPES: &[(&str, &str)] = &[
    ("genre_analysis",    "Genre Analysis"),
    ("full_report",       "Full Report"),
    ("kdp_keywords",      "KDP Keywords"),
    ("pr_keywords",       "PR Keywords"),
    ("competition_report","Competition Analysis"),
    ("category_finder",   "Category Finder"),
    ("genre_ranking",     "Genre Ranking"),
    ("mapped_categories", "Mapped Categories (Verified)"),
];

pub fn save_document(conn: &Connection, story_folder: &str, doc_type: &str, content: &str) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO story_documents (story_folder, doc_type, content, generated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(story_folder, doc_type) DO UPDATE SET content = excluded.content, generated_at = excluded.generated_at",
        params![story_folder, doc_type, content, now],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn get_document(conn: &Connection, story_folder: &str, doc_type: &str) -> Option<String> {
    conn.query_row(
        "SELECT content FROM story_documents WHERE story_folder = ?1 AND doc_type = ?2",
        params![story_folder, doc_type], |r| r.get(0)
    ).ok()
}

#[derive(serde::Serialize, Clone, Debug)]
pub struct DocMeta {
    pub doc_type:     String,
    pub label:        String,
    pub generated_at: String,
}

pub fn list_documents(conn: &Connection, story_folder: &str) -> Vec<DocMeta> {
    let mut stmt = match conn.prepare(
        "SELECT doc_type, generated_at FROM story_documents WHERE story_folder = ?1"
    ) { Ok(s) => s, Err(_) => return Vec::new() };

    let rows: Vec<(String, String)> = stmt.query_map(params![story_folder], |r| {
        Ok((r.get(0)?, r.get(1)?))
    }).and_then(|rows| rows.collect::<Result<Vec<_>, _>>()).unwrap_or_default();

    let mut out: Vec<DocMeta> = rows.into_iter().map(|(doc_type, generated_at)| {
        let label = DOC_TYPES.iter().find(|(t, _)| *t == doc_type)
            .map(|(_, l)| l.to_string())
            .unwrap_or_else(|| doc_type.clone());
        DocMeta { doc_type, label, generated_at }
    }).collect();

    out.sort_by_key(|d| DOC_TYPES.iter().position(|(t, _)| *t == d.doc_type).unwrap_or(99));
    out
}

// ── Tauri commands for the Reports panel ────────────────────────────

#[tauri::command]
pub async fn list_reports_cmd(db: tauri::State<'_, Db>, folder: String) -> Result<Vec<DocMeta>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    Ok(list_documents(&conn, &folder))
}

#[tauri::command]
pub async fn get_report_cmd(db: tauri::State<'_, Db>, folder: String, doc_type: String) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    get_document(&conn, &folder, &doc_type).ok_or_else(|| "Report not found.".to_string())
}
