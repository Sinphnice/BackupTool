use std::ffi::CStr;
use std::os::raw::c_char;

unsafe extern "C" {
    fn backup_core_version() -> *const c_char;
}

#[tauri::command]
fn core_version() -> Result<String, String> {
    let version = unsafe {
        let pointer = backup_core_version();
        if pointer.is_null() {
            return Err("backup_core_version returned null".to_string());
        }

        CStr::from_ptr(pointer)
    };

    version
        .to_str()
        .map(|value| value.to_string())
        .map_err(|error| format!("invalid UTF-8 from C++ core: {error}"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![core_version])
        .run(tauri::generate_context!())
        .expect("error while running Tauri application");
}

#[cfg(test)]
mod tests {
    use super::core_version;

    #[test]
    fn core_version_returns_cpp_value() {
        assert_eq!(core_version().unwrap(), "Backup Core 0.1.0");
    }
}
