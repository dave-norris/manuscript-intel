mod cancel;
#[allow(dead_code)]
mod canopy;
mod cdp;
mod commands;
mod category_finder;
mod competition_analyzer;
mod db;
mod genre_analyzer;
mod genre_taxonomy;
mod keyword_search;
mod models;
mod stories;
mod winningcat;

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
            genre_analyzer::match_categories_for_story,
            genre_analyzer::classify_bisac_for_story,
            genre_analyzer::find_genres_and_categories_for_story,
            genre_analyzer::analyze_story,
            genre_analyzer::rank_genres_for_story,
            genre_analyzer::verify_mapped_categories,
            genre_taxonomy::get_genre_taxonomy,
            db::list_genres_cmd,
            db::add_kdp_path_cmd,
            db::list_reports_cmd,
            db::get_report_cmd,
            db::save_report_version_cmd,
            db::list_saved_reports_cmd,
            db::get_saved_report_cmd,
            db::delete_saved_report_cmd,
            winningcat::import_winningcat_csv,
            winningcat::remove_stale_kdp_categories,
            keyword_search::search_pr_keywords,
            competition_analyzer::analyze_competition,
            cancel::cancel_operation,
            canopy::test_canopy_connection,
            canopy::analyze_categories_canopy,
            canopy::analyze_competition_canopy,
            canopy::search_keywords_canopy,
            canopy::mine_competitor_reviews,
            canopy::analyze_comp_authors,
            canopy::sync_categories_canopy,
            canopy::deep_category_analysis,
            stories::list_stories,
            stories::add_story,
            stories::update_story,
            stories::delete_story,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
