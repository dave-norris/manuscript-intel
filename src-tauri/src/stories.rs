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
    #[serde(default)]
    pub bible_path: String,
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
pub struct InitStoryRequest {
    pub name:          String,
    pub parent_folder: String,
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
    #[serde(default)]
    pub bible_path: String,
}

/// Turn a story name into a safe single path segment.
fn sanitize_folder_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_string();
    if cleaned.is_empty() {
        "Untitled Story".to_string()
    } else {
        cleaned
    }
}

fn ensure_story_scaffold(story_dir: &std::path::Path) -> Result<(), String> {
    let structure = crate::folder_structure::current();
    for sub in structure.scaffold_dirs() {
        fs::create_dir_all(story_dir.join(sub))
            .map_err(|e| format!("Cannot create {}/: {}", sub, e))?;
    }
    Ok(())
}

/// List all stories.
#[tauri::command]
pub async fn list_stories(app: AppHandle) -> StoriesResult {
    match load_stories(&app) {
        Ok(stories) => StoriesResult { success: true, stories, error: String::new() },
        Err(e)      => StoriesResult { success: false, stories: Vec::new(), error: e },
    }
}

fn register_story(app: &AppHandle, name: String, folder: String) -> StoriesResult {
    let now = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let story = Story {
        id:         new_id(),
        name,
        folder,
        created:    now,
        bible_path: String::new(),
    };

    match load_stories(app) {
        Err(e) => StoriesResult { success: false, stories: Vec::new(), error: e },
        Ok(mut stories) => {
            stories.push(story);
            match save_stories(app, &stories) {
                Err(e) => StoriesResult { success: false, stories: Vec::new(), error: e },
                Ok(_)  => StoriesResult { success: true, stories, error: String::new() },
            }
        }
    }
}

/// Register an existing story folder (does not create folders).
#[tauri::command]
pub async fn add_story(app: AppHandle, request: AddStoryRequest) -> StoriesResult {
    let folder = PathBuf::from(&request.folder);
    if !folder.is_dir() {
        return StoriesResult {
            success: false, stories: Vec::new(),
            error: format!("Folder does not exist: {}", request.folder),
        };
    }
    register_story(&app, request.name.trim().to_string(), request.folder.clone())
}

/// Create a new empty story folder named after the story, with the configured
/// subfolders from Settings → Folder Structure, then register it.
#[tauri::command]
pub async fn init_story(app: AppHandle, request: InitStoryRequest) -> StoriesResult {
    // Ensure cache matches disk before scaffolding
    let _ = crate::folder_structure::load(&app);
    let name = request.name.trim().to_string();
    if name.is_empty() {
        return StoriesResult {
            success: false, stories: Vec::new(),
            error: "Please enter a story name.".to_string(),
        };
    }

    let parent = PathBuf::from(&request.parent_folder);
    if !parent.is_dir() {
        return StoriesResult {
            success: false, stories: Vec::new(),
            error: format!("Parent folder does not exist: {}", request.parent_folder),
        };
    }

    let story_dir = parent.join(sanitize_folder_name(&name));
    if story_dir.exists() {
        return StoriesResult {
            success: false, stories: Vec::new(),
            error: format!(
                "A folder already exists at: {}",
                story_dir.to_string_lossy()
            ),
        };
    }

    if let Err(e) = ensure_story_scaffold(&story_dir) {
        // Best-effort cleanup if we partially created the tree
        let _ = fs::remove_dir_all(&story_dir);
        return StoriesResult { success: false, stories: Vec::new(), error: e };
    }

    register_story(
        &app,
        name,
        story_dir.to_string_lossy().to_string(),
    )
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

    match load_stories(&app) {
        Err(e) => StoriesResult { success: false, stories: Vec::new(), error: e },
        Ok(mut stories) => {
            if let Some(s) = stories.iter_mut().find(|s| s.id == request.id) {
                s.name   = request.name.trim().to_string();
                s.folder = request.folder.clone();
                s.bible_path = request.bible_path.clone();
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
