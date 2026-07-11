// genre_taxonomy.rs — thin wrapper over the database's genre table.
//
// The master genre list and the genre-to-KDP-path map now live in SQLite
// (see db.rs) — this file just exposes them to the rest of the app and to
// the frontend. The JSON files in src-tauri/data/ are used ONLY as one-time
// seed data by db.rs on first launch; after that, the database is
// authoritative and grows on its own as Category Finder discovers real paths.

use crate::db::{self, Db, GenreRow};

/// Everything needed to build the genre-ranking AI prompt: name + description
/// for every genre currently known to the database.
pub fn master_genre_list(db: &Db) -> Result<Vec<GenreRow>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    db::list_genres(&conn)
}

/// Known KDP path(s) for a genre name, in the given store.
pub fn kdp_paths_for_genre(db: &Db, genre_name: &str, store: &str) -> Result<Vec<String>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    db::kdp_paths_for_genre(&conn, genre_name, store)
}

/// Exposed to the frontend for reference/debugging — the live master list a
/// manuscript is scored against.
#[tauri::command]
pub async fn get_genre_taxonomy(db: tauri::State<'_, Db>) -> Result<Vec<GenreRow>, String> {
    master_genre_list(&db)
}
