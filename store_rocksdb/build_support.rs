use std::{env, path::PathBuf};

pub fn workspace_lock_path() -> Option<PathBuf> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").ok()?);
    manifest_dir.ancestors().find_map(|dir| {
        let candidate = dir.join("Cargo.lock");
        if candidate.exists() {
            Some(candidate)
        } else {
            None
        }
    })
}

pub fn find_version(contents: &str, package: &str) -> Option<String> {
    let mut in_package = false;
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed == "[[package]]" {
            in_package = false;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("name = \"") {
            let end = rest.strip_suffix('"')?;
            in_package = end == package;
            continue;
        }
        if in_package {
            if let Some(rest) = trimmed.strip_prefix("version = \"") {
                let version = rest.strip_suffix('"')?.to_string();
                return Some(version);
            }
        }
    }
    None
}
