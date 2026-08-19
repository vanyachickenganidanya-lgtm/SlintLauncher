use std::path::{Path, PathBuf};

use crate::error::{io_err, Result};

pub fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("SlintLauncher")
}

pub fn config_path() -> PathBuf {
    data_dir().join("config.json")
}

pub fn minecraft_dir() -> PathBuf {
    data_dir().join("minecraft")
}

pub fn versions_dir() -> PathBuf {
    minecraft_dir().join("versions")
}

pub fn libraries_dir() -> PathBuf {
    minecraft_dir().join("libraries")
}

pub fn assets_dir() -> PathBuf {
    minecraft_dir().join("assets")
}

pub fn instances_dir() -> PathBuf {
    data_dir().join("instances")
}

pub fn instance_game_dir(instance_id: &str) -> PathBuf {
    instances_dir().join(instance_id).join("minecraft")
}

pub fn version_dir(version_id: &str) -> PathBuf {
    versions_dir().join(version_id)
}

pub fn version_json(version_id: &str) -> PathBuf {
    version_dir(version_id).join(format!("{version_id}.json"))
}

pub fn version_jar(version_id: &str) -> PathBuf {
    version_dir(version_id).join(format!("{version_id}.jar"))
}

pub fn natives_dir(version_id: &str) -> PathBuf {
    version_dir(version_id).join("natives")
}

pub fn ensure_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).map_err(|source| io_err(path, source))
}

pub fn classpath_sep() -> &'static str {
    if cfg!(windows) {
        ";"
    } else {
        ":"
    }
}
