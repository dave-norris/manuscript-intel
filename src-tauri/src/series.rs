// series.rs — Series management (groups stories into reading order)

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use crate::db;

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Series {
    pub id: i64,
    pub name: String,
    pub created_at: String,
    pub books: Vec<SeriesBook>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SeriesBook {
    pub story_folder: String,
    pub story_name: String,
    pub book_order: i64,
}

#[derive(Serialize)]
pub struct SeriesResult {
    pub success: bool,
    pub series: Vec<Series>,
    pub error: String,
}

#[derive(Deserialize)]
pub struct CreateSeriesRequest {
    pub name: String,
    pub books: Vec<SeriesBookInput>,
}

#[derive(Deserialize)]
pub struct UpdateSeriesRequest {
    pub id: i64,
    pub name: String,
    pub books: Vec<SeriesBookInput>,
}

#[derive(Deserialize, Clone)]
pub struct SeriesBookInput {
    pub story_folder: String,
    pub story_name: String,
    pub book_order: i64,
}

// ── Tauri commands ────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_series(app: AppHandle) -> SeriesResult {
    let database = app.state::<db::Db>();
    let conn = database.0.lock().unwrap();

    let mut stmt = match conn.prepare("SELECT id, name, created_at FROM series ORDER BY name") {
        Ok(s) => s,
        Err(e) => return SeriesResult { success: false, series: Vec::new(), error: e.to_string() },
    };

    let series_rows: Vec<(i64, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
        .unwrap_or_default();

    let mut series_list = Vec::new();
    for (id, name, created_at) in series_rows {
        let books = load_series_books(&conn, id);
        series_list.push(Series { id, name, created_at, books });
    }

    SeriesResult { success: true, series: series_list, error: String::new() }
}

#[tauri::command]
pub async fn create_series(app: AppHandle, request: CreateSeriesRequest) -> SeriesResult {
    {
        let database = app.state::<db::Db>();
        let conn = database.0.lock().unwrap();

        let name = request.name.trim();
        if name.is_empty() {
            return SeriesResult { success: false, series: Vec::new(), error: "Series name is required.".to_string() };
        }

        let now = chrono::Utc::now().to_rfc3339();
        if let Err(e) = conn.execute(
            "INSERT INTO series (name, created_at) VALUES (?1, ?2)",
            rusqlite::params![name, now],
        ) {
            return SeriesResult { success: false, series: Vec::new(), error: format!("Could not create series: {}", e) };
        }

        let series_id = conn.last_insert_rowid();
        save_series_books(&conn, series_id, &request.books);
    }
    list_series(app).await
}

#[tauri::command]
pub async fn update_series(app: AppHandle, request: UpdateSeriesRequest) -> SeriesResult {
    {
        let database = app.state::<db::Db>();
        let conn = database.0.lock().unwrap();

        let name = request.name.trim();
        if name.is_empty() {
            return SeriesResult { success: false, series: Vec::new(), error: "Series name is required.".to_string() };
        }

        if let Err(e) = conn.execute(
            "UPDATE series SET name = ?1 WHERE id = ?2",
            rusqlite::params![name, request.id],
        ) {
            return SeriesResult { success: false, series: Vec::new(), error: format!("Could not update series: {}", e) };
        }

        save_series_books(&conn, request.id, &request.books);
    }
    list_series(app).await
}

#[tauri::command]
pub async fn delete_series(app: AppHandle, id: i64) -> SeriesResult {
    {
        let database = app.state::<db::Db>();
        let conn = database.0.lock().unwrap();
        let _ = conn.execute("DELETE FROM series_books WHERE series_id = ?1", rusqlite::params![id]);
        let _ = conn.execute("DELETE FROM series WHERE id = ?1", rusqlite::params![id]);
    }
    list_series(app).await
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn load_series_books(conn: &rusqlite::Connection, series_id: i64) -> Vec<SeriesBook> {
    let mut stmt = match conn.prepare(
        "SELECT story_folder, story_name, book_order FROM series_books WHERE series_id = ?1 ORDER BY book_order"
    ) { Ok(s) => s, Err(_) => return Vec::new() };

    stmt.query_map(rusqlite::params![series_id], |r| {
        Ok(SeriesBook {
            story_folder: r.get(0)?,
            story_name: r.get(1)?,
            book_order: r.get(2)?,
        })
    }).and_then(|rows| rows.collect::<Result<Vec<_>, _>>()).unwrap_or_default()
}

fn save_series_books(conn: &rusqlite::Connection, series_id: i64, books: &[SeriesBookInput]) {
    let _ = conn.execute("DELETE FROM series_books WHERE series_id = ?1", rusqlite::params![series_id]);
    for book in books {
        let _ = conn.execute(
            "INSERT INTO series_books (series_id, story_folder, story_name, book_order) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![series_id, book.story_folder, book.story_name, book.book_order],
        );
    }
}
