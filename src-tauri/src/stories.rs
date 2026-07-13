// stories.rs — Persistent story registry
//
// Stores story metadata in:
//   ~/Library/Application Support/manuscript-intel/stories.json
//
// Each story has: id, name, folder path, created date.

use std::fs;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri::Manager;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Story {
    pub id:      String,
    pub name:    String,
    pub folder:  String,
    pub created: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct StoriesFile {
    stories: Vec<Story>,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn app_data_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|e| format!("Cannot resolve app data dir: {}", e))
}

fn stories_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app_data_path(app)?;
    fs::create_dir_all(&dir)
        .map_err(|e| format!("Cannot create app data dir: {}", e))?;
    Ok(dir.join("stories.json"))
}

fn load_stories(app: &AppHandle) -> Result<Vec<Story>, String> {
    let path = stories_path(app)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(&path)
        .map_err(|e| format!("Cannot read stories.json: {}", e))?;
    let sf: StoriesFile = serde_json::from_str(&raw)
        .map_err(|e| format!("Cannot parse stories.json: {}", e))?;
    Ok(sf.stories)
}

fn save_stories(app: &AppHandle, stories: &[Story]) -> Result<(), String> {
    let path = stories_path(app)?;
    let sf = StoriesFile { stories: stories.to_vec() };
    let json = serde_json::to_string_pretty(&sf)
        .map_err(|e| format!("Cannot serialize stories: {}", e))?;
    fs::write(&path, json)
        .map_err(|e| format!("Cannot write stories.json: {}", e))?;
    Ok(())
}

fn new_id() -> String {
    // Simple timestamp-based ID — no uuid crate needed
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{:x}", ts)
}

// ── Commands ──────────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct StoriesResult {
    pub success: bool,
    pub stories: Vec<Story>,
    pub error:   String,
}

#[derive(Deserialize)]
pub struct AddStoryRequest {
    pub name:   String,
    pub folder: String,
}

#[derive(Deserialize)]
pub struct UpdateStoryRequest {
    pub id:     String,
    pub name:   String,
    pub folder: String,
}

/// List all stories.
#[tauri::command]
pub async fn list_stories(app: AppHandle) -> StoriesResult {
    match load_stories(&app) {
        Ok(stories) => StoriesResult { success: true, stories, error: String::new() },
        Err(e)      => StoriesResult { success: false, stories: Vec::new(), error: e },
    }
}

/// Add a new story. Creates _analysis/ folder inside the story folder.
#[tauri::command]
pub async fn add_story(app: AppHandle, request: AddStoryRequest) -> StoriesResult {
    let folder = PathBuf::from(&request.folder);

    // Validate folder exists
    if !folder.exists() {
        return StoriesResult {
            success: false, stories: Vec::new(),
            error: format!("Folder does not exist: {}", request.folder),
        };
    }

    // Create _analysis/ subfolder
    let analysis_dir = folder.join("_analysis");
    if let Err(e) = fs::create_dir_all(&analysis_dir) {
        return StoriesResult {
            success: false, stories: Vec::new(),
            error: format!("Cannot create _analysis folder: {}", e),
        };
    }

    let now = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let story = Story {
        id:      new_id(),
        name:    request.name.trim().to_string(),
        folder:  request.folder.clone(),
        created: now,
    };

    match load_stories(&app) {
        Err(e) => StoriesResult { success: false, stories: Vec::new(), error: e },
        Ok(mut stories) => {
            stories.push(story);
            match save_stories(&app, &stories) {
                Err(e) => StoriesResult { success: false, stories: Vec::new(), error: e },
                Ok(_)  => StoriesResult { success: true, stories, error: String::new() },
            }
        }
    }
}

/// Update a story's name and/or folder.
#[tauri::command]
pub async fn update_story(app: AppHandle, request: UpdateStoryRequest) -> StoriesResult {
    let folder = PathBuf::from(&request.folder);
    if !folder.exists() {
        return StoriesResult {
            success: false, stories: Vec::new(),
            error: format!("Folder does not exist: {}", request.folder),
        };
    }

    // Ensure _analysis/ exists
    let _ = fs::create_dir_all(folder.join("_analysis"));

    match load_stories(&app) {
        Err(e) => StoriesResult { success: false, stories: Vec::new(), error: e },
        Ok(mut stories) => {
            if let Some(s) = stories.iter_mut().find(|s| s.id == request.id) {
                s.name   = request.name.trim().to_string();
                s.folder = request.folder.clone();
            }
            match save_stories(&app, &stories) {
                Err(e) => StoriesResult { success: false, stories: Vec::new(), error: e },
                Ok(_)  => StoriesResult { success: true, stories, error: String::new() },
            }
        }
    }
}

/// Delete a story by id (does NOT delete the folder).
#[tauri::command]
pub async fn delete_story(app: AppHandle, id: String) -> StoriesResult {
    match load_stories(&app) {
        Err(e) => StoriesResult { success: false, stories: Vec::new(), error: e },
        Ok(mut stories) => {
            stories.retain(|s| s.id != id);
            match save_stories(&app, &stories) {
                Err(e) => StoriesResult { success: false, stories: Vec::new(), error: e },
                Ok(_)  => StoriesResult { success: true, stories, error: String::new() },
            }
        }
    }
}
