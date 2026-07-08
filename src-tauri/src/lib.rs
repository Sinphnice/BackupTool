mod commands;
#[cfg(test)]
mod commands_tests;
mod dto;

/// 桌面端和移动端共用的 Tauri 应用入口。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::backup,
            commands::list_snapshots,
            commands::restore
        ])
        .run(tauri::generate_context!())
        .expect("error while running Tauri application");
}
