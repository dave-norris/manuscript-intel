mod cancel;
mod cdp;
mod commands;
mod category_finder;
mod competition_analyzer;
mod db;
mod genre_analyzer;
mod genre_taxonomy;
mod models;
mod stories;

use tauri::Manager;

pub use cancel::{is_cancelled, reset as reset_cancel};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let database = db::init(&handle).expect("failed to initialize database");
            app.manage(database);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::check_rocket_status,
            commands::launch_rocket,
            commands::analyze_categories,
            commands::analyze_csv,
            commands::find_categories,
            commands::list_models,
            genre_analyzer::pick_manuscript_folder,
            genre_analyzer::generate_summaries,
            genre_analyzer::run_everything,
            genre_analyzer::analyze_genre,
            genre_analyzer::run_full_analysis,
            genre_analyzer::optimize_keywords,
            genre_analyzer::generate_pr_keywords,
            genre_analyzer::check_analysis_state,
            genre_analyzer::find_categories_for_story,
            genre_analyzer::rank_genres_for_story,
            genre_analyzer::verify_mapped_categories,
            genre_taxonomy::get_genre_taxonomy,
            db::list_genres_cmd,
            db::add_kdp_path_cmd,
            db::list_reports_cmd,
            db::get_report_cmd,
            competition_analyzer::analyze_competition,
            cancel::cancel_operation,
            stories::list_stories,
            stories::add_story,
            stories::update_story,
            stories::delete_story,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
