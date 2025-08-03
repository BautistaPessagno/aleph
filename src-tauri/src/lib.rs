// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/

mod apps;
mod files;
mod icons;

//opener
use opener;

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[tauri::command]
fn open_path(path: &str) -> Result<(), String> {
    opener::open(path).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            greet,
            files::search_index,
            open_path,
            apps::app_search
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
