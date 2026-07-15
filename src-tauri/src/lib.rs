#![deny(dead_code)]

mod analysis;
mod cancel;
mod canopy;
mod commands;
mod competition_analyzer;
mod dataforseo;
mod db;
mod genre_taxonomy;
mod models;
mod stories;
mod winningcat;

use tauri::Manager;

pub use cancel::{is_cancelled, reset as reset_cancel, notify as cancel_notify};

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
            commands::analyze_csv,
            commands::list_models,
            analysis::chapters::generate_summaries,
            analysis::genres::analyze_genre,
            analysis::genres::rank_genres_for_story,
            analysis::categories::find_categories_for_story,
            analysis::categories::match_categories_for_story,
            analysis::categories::verify_mapped_categories,
            analysis::bisac::classify_bisac_for_story,
            analysis::keywords::generate_search_terms,
            analysis::keywords::optimize_keywords,
            analysis::pipeline::pick_manuscript_folder,
            analysis::pipeline::check_analysis_state,
            analysis::pipeline::run_everything,
            analysis::pipeline::run_full_analysis,
            analysis::pipeline::find_genres_and_categories_for_story,
            analysis::pipeline::analyze_story,
            analysis::zeigarnik::analyze_zeigarnik_for_story,
            analysis::continuity::check_continuity_for_story,
            analysis::continuity::check_continuity_for_series,
            genre_taxonomy::get_genre_taxonomy,
            db::list_genres_cmd,
            db::list_report_types_cmd,
            db::add_kdp_path_cmd,
            db::list_reports_cmd,
            db::save_activity_log_cmd,
            db::get_report_cmd,
            db::delete_report_cmd,
            db::list_series_cmd,
            db::create_series_cmd,
            db::delete_series_cmd,
            db::list_series_books_cmd,
            db::add_story_to_series_cmd,
            db::remove_story_from_series_cmd,
            winningcat::import_winningcat_csv,
            winningcat::remove_stale_kdp_categories,
            cancel::cancel_operation,
            canopy::test_canopy_connection,
            canopy::analyze_categories_canopy,
            canopy::analyze_competition_canopy,
            canopy::search_keywords_canopy,
            canopy::mine_competitor_reviews,
            canopy::analyze_comp_authors,
            canopy::deep_category_analysis,
            dataforseo::test_dataforseo_connection,
            dataforseo::search_amazon_keywords,
            dataforseo::search_google_keywords,
            stories::list_stories,
            stories::add_story,
            stories::update_story,
            stories::delete_story,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
