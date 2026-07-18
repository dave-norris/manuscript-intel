// folder_structure.rs — Configurable story folder layout
//
// The app only cares about three purpose folders:
//   manuscript — chapter .md files (analysis)
//   bible      — story bible docs
//   characters — character docs (merged into bible discovery)
//
// Everything else is an optional scaffold path (`extra`) that Create empty
// story will create, but the app never reads specially.
//
// Stored in SQLite (`folder_structure` table). Cached in-process.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri::Manager;

use crate::db::Db;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FolderStructure {
    /// Chapter `.md` files (analysis reads only here)
    pub manuscript: String,
    /// Story bible docs
    pub bible: String,
    /// Character docs (merged into bible discovery)
    pub characters: String,
    /// Additional relative paths created on "Create empty story" (app ignores them)
    #[serde(default)]
    pub extra: Vec<String>,
}

impl Default for FolderStructure {
    fn default() -> Self {
        Self {
            manuscript: "Manuscript".into(),
            bible: "Bible".into(),
            characters: "Characters".into(),
            // User-wanted defaults; not used by app logic
            extra: vec!["Publishing/Cover".into(), "Research".into()],
        }
    }
}

impl FolderStructure {
    pub fn manuscript(&self) -> &str {
        if self.manuscript.trim().is_empty() { "Manuscript" } else { self.manuscript.as_str() }
    }

    pub fn bible(&self) -> &str {
        if self.bible.trim().is_empty() { "Bible" } else { self.bible.as_str() }
    }

    pub fn characters(&self) -> &str {
        if self.characters.trim().is_empty() { "Characters" } else { self.characters.as_str() }
    }

    /// All relative paths created when scaffolding a new story.
    pub fn scaffold_dirs(&self) -> Vec<&str> {
        let mut dirs = vec![self.manuscript(), self.bible(), self.characters()];
        for p in &self.extra {
            let t = p.trim();
            if !t.is_empty() {
                dirs.push(t);
            }
        }
        dirs
    }

    pub fn keep_empty_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        for path in self.scaffold_dirs() {
            for seg in path.split(['/', '\\']) {
                let s = seg.trim();
                if !s.is_empty() {
                    names.push(s.to_ascii_lowercase());
                }
            }
        }
        names
    }

    fn sanitize_path(value: &str, fallback: &str) -> String {
        let trimmed = value.trim().trim_matches('/').trim_matches('\\');
        if trimmed.is_empty() || trimmed.contains("..") {
            fallback.to_string()
        } else {
            trimmed.replace('\\', "/")
        }
    }

    pub fn normalized(mut self) -> Self {
        let defaults = Self::default();
        self.manuscript = Self::sanitize_path(&self.manuscript, &defaults.manuscript);
        self.bible = Self::sanitize_path(&self.bible, &defaults.bible);
        self.characters = Self::sanitize_path(&self.characters, &defaults.characters);
        self.extra = self
            .extra
            .into_iter()
            .filter_map(|p| {
                let trimmed = p.trim().trim_matches('/').trim_matches('\\');
                if trimmed.is_empty() || trimmed.contains("..") {
                    None
                } else {
                    Some(trimmed.replace('\\', "/"))
                }
            })
            .collect();
        self
    }
}

fn cache() -> &'static Mutex<FolderStructure> {
    static CACHE: OnceLock<Mutex<FolderStructure>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(FolderStructure::default()))
}

pub fn current() -> FolderStructure {
    cache().lock().map(|g| g.clone()).unwrap_or_default()
}

fn set_current(structure: FolderStructure) {
    if let Ok(mut g) = cache().lock() {
        *g = structure;
    }
}

fn legacy_json_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("folder_structure.json"))
}

#[derive(Deserialize)]
struct LegacyFlat {
    manuscript: Option<String>,
    bible: Option<String>,
    characters: Option<String>,
    publishing: Option<String>,
    research: Option<String>,
    #[serde(default)]
    extra: Vec<String>,
}

#[derive(Deserialize)]
struct LegacyListEntry {
    path: String,
    #[serde(default)]
    role: String,
}

#[derive(Deserialize)]
struct LegacyList {
    folders: Vec<LegacyListEntry>,
}

fn from_legacy_list(folders: Vec<LegacyListEntry>) -> FolderStructure {
    let mut s = FolderStructure {
        manuscript: "Manuscript".into(),
        bible: "Bible".into(),
        characters: "Characters".into(),
        extra: Vec::new(),
    };
    for e in folders {
        let path = e.path.trim();
        if path.is_empty() {
            continue;
        }
        match e.role.trim().to_ascii_lowercase().as_str() {
            "manuscript" => s.manuscript = path.into(),
            "bible" => s.bible = path.into(),
            "characters" => s.characters = path.into(),
            // Former purpose roles become extras
            "publishing" | "research" | _ => s.extra.push(path.replace('\\', "/")),
        }
    }
    s.normalized()
}

fn read_from_db(conn: &Connection) -> Result<FolderStructure, String> {
    let mut stmt = conn
        .prepare(
            "SELECT path, role FROM folder_structure ORDER BY sort_order ASC, id ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?;

    let mut s = FolderStructure {
        manuscript: "Manuscript".into(),
        bible: "Bible".into(),
        characters: "Characters".into(),
        extra: Vec::new(),
    };
    let mut any = false;
    for row in rows {
        let (path, role) = row.map_err(|e| e.to_string())?;
        any = true;
        let path = path.trim();
        if path.is_empty() {
            continue;
        }
        match role.trim().to_ascii_lowercase().as_str() {
            "manuscript" => s.manuscript = path.into(),
            "bible" => s.bible = path.into(),
            "characters" => s.characters = path.into(),
            // publishing/research and anything else → extra
            _ => s.extra.push(path.replace('\\', "/")),
        }
    }
    if !any {
        return Ok(FolderStructure::default());
    }
    Ok(s.normalized())
}

fn write_to_db(conn: &Connection, structure: &FolderStructure) -> Result<(), String> {
    conn.execute("DELETE FROM folder_structure", [])
        .map_err(|e| e.to_string())?;

    let mut rows: Vec<(String, String)> = vec![
        (structure.manuscript().to_string(), "manuscript".into()),
        (structure.bible().to_string(), "bible".into()),
        (structure.characters().to_string(), "characters".into()),
    ];
    for p in &structure.extra {
        let t = p.trim();
        if !t.is_empty() {
            rows.push((t.to_string(), String::new()));
        }
    }

    for (i, (path, role)) in rows.iter().enumerate() {
        conn.execute(
            "INSERT INTO folder_structure (path, role, sort_order) VALUES (?1, ?2, ?3)",
            params![path, role, i as i64],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn count_rows(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM folder_structure", [], |r| r.get(0))
        .unwrap_or(0)
}

fn migrate_legacy_json(app: &AppHandle) -> Option<FolderStructure> {
    let path = legacy_json_path(app)?;
    if !path.exists() {
        return None;
    }
    let raw = fs::read_to_string(&path).ok()?;
    let structure = if let Ok(list) = serde_json::from_str::<LegacyList>(&raw) {
        from_legacy_list(list.folders)
    } else if let Ok(flat) = serde_json::from_str::<LegacyFlat>(&raw) {
        let mut extra = flat.extra;
        if let Some(p) = flat.publishing {
            if !p.trim().is_empty() {
                extra.push(p);
            }
        }
        if let Some(p) = flat.research {
            if !p.trim().is_empty() {
                extra.push(p);
            }
        }
        FolderStructure {
            manuscript: flat.manuscript.unwrap_or_else(|| "Manuscript".into()),
            bible: flat.bible.unwrap_or_else(|| "Bible".into()),
            characters: flat.characters.unwrap_or_else(|| "Characters".into()),
            extra,
        }
        .normalized()
    } else if let Ok(s) = serde_json::from_str::<FolderStructure>(&raw) {
        s.normalized()
    } else {
        return None;
    };
    let _ = fs::remove_file(&path);
    Some(structure)
}

pub fn load(app: &AppHandle) -> FolderStructure {
    let db = app.state::<Db>();
    let conn = match db.0.lock() {
        Ok(c) => c,
        Err(_) => {
            let s = FolderStructure::default();
            set_current(s.clone());
            return s;
        }
    };

    let structure = if count_rows(&conn) == 0 {
        let seeded = migrate_legacy_json(app).unwrap_or_default().normalized();
        let _ = write_to_db(&conn, &seeded);
        seeded
    } else {
        let s = read_from_db(&conn).unwrap_or_default();
        // Normalize & rewrite so publishing/research roles become extras
        let s = s.normalized();
        let _ = write_to_db(&conn, &s);
        s
    };

    set_current(structure.clone());
    structure
}

pub fn save(app: &AppHandle, structure: FolderStructure) -> Result<FolderStructure, String> {
    let structure = structure.normalized();
    let db = app.state::<Db>();
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    write_to_db(&conn, &structure)?;
    set_current(structure.clone());
    Ok(structure)
}

pub fn resolve_subdir(root: &Path, relative: &str) -> Option<PathBuf> {
    let relative = relative.trim().trim_matches('/').trim_matches('\\');
    if relative.is_empty() {
        return None;
    }

    let exact = root.join(relative);
    if exact.is_dir() {
        return Some(exact);
    }

    let mut current = root.to_path_buf();
    for seg in relative.split(['/', '\\']) {
        if seg.is_empty() {
            continue;
        }
        let candidate = current.join(seg);
        if candidate.is_dir() {
            current = candidate;
            continue;
        }
        let Ok(entries) = fs::read_dir(&current) else {
            return None;
        };
        let mut found = None;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.eq_ignore_ascii_case(seg) && entry.path().is_dir() {
                found = Some(entry.path());
                break;
            }
        }
        current = found?;
    }
    if current.is_dir() {
        Some(current)
    } else {
        None
    }
}

pub fn is_hidden_story_dir(name: &str) -> bool {
    name.eq_ignore_ascii_case("_analysis")
}

#[tauri::command]
pub async fn get_folder_structure(app: AppHandle) -> FolderStructure {
    load(&app)
}

#[tauri::command]
pub async fn save_folder_structure(
    app: AppHandle,
    structure: FolderStructure,
) -> Result<FolderStructure, String> {
    save(&app, structure)
}
