use std::fs;

mod build_support;

fn main() {
    if let Some(lock_path) = build_support::workspace_lock_path() {
        if let Ok(contents) = fs::read_to_string(lock_path) {
            if let Some(version) = build_support::find_version(&contents, "librocksdb-sys") {
                let parsed = version
                    .split('+')
                    .nth(1)
                    .unwrap_or(version.as_str())
                    .to_string();
                println!("cargo:rustc-env=RSN_ROCKSDB_LIB_VERSION={parsed}");
            }
        }
    }
}
