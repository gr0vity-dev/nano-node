use rsnano_types::Networks;
use std::path::PathBuf;
use uuid::Uuid;

pub fn working_path_for(network: Networks) -> Option<PathBuf> {
    if let Ok(path_override) = std::env::var("NANO_APP_PATH") {
        eprintln!(
            "Application path overridden by NANO_APP_PATH environment variable: {path_override}"
        );
        return Some(path_override.into());
    }

    dirs::home_dir().and_then(|mut path| {
        let subdir = match network {
            Networks::Invalid => return None,
            Networks::NanoDevNetwork => "NanoDev",
            Networks::NanoBetaNetwork => "NanoBeta",
            Networks::NanoLiveNetwork => "Nano",
            Networks::NanoTestNetwork => "NanoTest",
        };
        path.push(subdir);
        Some(path)
    })
}

pub fn unique_path() -> Option<PathBuf> {
    unique_path_for(Networks::NanoDevNetwork)
}

fn unique_path_for(network: Networks) -> Option<PathBuf> {
    if let Some(mut path) = working_path_for(network) {
        path.push(Uuid::new_v4().to_string());
        if std::fs::create_dir_all(&path).is_ok() {
            return Some(path);
        }
    }

    let mut fallback = std::env::temp_dir();
    fallback.push(format!("rsnano-{}", Uuid::new_v4()));
    if std::fs::create_dir_all(&fallback).is_ok() {
        return Some(fallback);
    }

    None
}
