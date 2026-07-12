mod commands;
#[cfg(test)]
mod commands_tests;
mod dto;

/// 桌面端和移动端共用的 Tauri 应用入口。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            commands::backup,
            commands::change_repository_password,
            commands::create_repository,
            commands::delete_repository,
            commands::delete_snapshot,
            commands::export_repository,
            commands::import_repository,
            commands::list_snapshots,
            commands::open_repository,
            commands::rename_repository,
            commands::unlock_repository,
            commands::restore
        ])
        .run(tauri::generate_context!())
        .expect("error while running Tauri application");
}
