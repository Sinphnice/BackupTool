mod commands;
mod dto;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            commands::backup,
            commands::restore
        ])
        .run(tauri::generate_context!())
        .expect("error while running Tauri application");
}
