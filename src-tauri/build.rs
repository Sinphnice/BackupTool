use std::env;
use std::path::{Path, PathBuf};

fn link_search(path: &Path) {
    println!("cargo:rustc-link-search=native={}", path.display());
}

fn main() {
    println!("cargo:rerun-if-changed=../core/CMakeLists.txt");
    println!("cargo:rerun-if-changed=../core/include/backup/version.hpp");
    println!("cargo:rerun-if-changed=../core/include/backup_c_api.h");
    println!("cargo:rerun-if-changed=../core/src/version.cpp");
    println!("cargo:rerun-if-changed=../core/src/backup_c_api.cpp");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
    let (build_dir, config) = if profile == "release" {
        (manifest_dir.join("../core/build/msvc-release"), "Release")
    } else {
        (manifest_dir.join("../core/build/msvc-debug"), "Debug")
    };

    let library_dir = build_dir.join("core").join(config);
    let library_name = if cfg!(target_os = "windows") {
        "backup_core.lib"
    } else {
        "libbackup_core.a"
    };
    let library_path = library_dir.join(library_name);

    if !library_path.exists() {
        panic!(
            "C++ core library not found at {}. Run `just core-build-{}` first.",
            library_path.display(),
            profile
        );
    }

    link_search(&library_dir);

    println!("cargo:rustc-link-lib=static=backup_core");

    let target = env::var("TARGET").unwrap_or_default();
    if target.contains("windows-gnu") {
        println!("cargo:rustc-link-lib=dylib=stdc++");
    }

    tauri_build::build();
}
