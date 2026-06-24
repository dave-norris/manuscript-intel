mod cdp;
mod commands;
mod category_finder;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .invoke_handler(tauri::generate_handler![
            commands::check_rocket_status,
            commands::launch_rocket,
            commands::analyze_categories,
            commands::analyze_csv,
            commands::find_categories,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
