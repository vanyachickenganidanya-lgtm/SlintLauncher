use crate::util::data_dir;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

fn default_memory() -> u32 {
    4096
}
fn default_width() -> u32 {
    854
}
fn default_height() -> u32 {
    480
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub java_path: String,
    #[serde(default = "default_memory")]
    pub max_memory: u32,
    pub jvm_args: String,
    #[serde(default = "default_width")]
    pub window_width: u32,
    #[serde(default = "default_height")]
    pub window_height: u32,
    pub fullscreen: bool,
    #[serde(default = "default_true")]
    pub hide_launcher: bool,
    #[serde(default = "default_true")]
    pub auto_java: bool,
    #[serde(default = "default_true")]
    pub russian: bool,
    pub light_theme: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            java_path: String::new(),
            max_memory: default_memory(),
            jvm_args: String::from("-XX:+UseG1GC -XX:+UnlockExperimentalVMOptions -Dfile.encoding=UTF-8"),
            window_width: default_width(),
            window_height: default_height(),
            fullscreen: false,
            hide_launcher: true,
            auto_java: true,
            russian: true,
            light_theme: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instance {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub last_played: String,
    #[serde(default)]
    pub folder: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub username: String,
    pub uuid: String,
    /// "ely" or "offline"
    pub kind: String,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub client_token: String,
    #[serde(default)]
    pub skin_url: String,
}

impl Account {
    pub fn is_ely(&self) -> bool {
        self.kind == "ely"
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    pub settings: Settings,
    pub instances: Vec<Instance>,
    pub accounts: Vec<Account>,
    pub active_account: String,
}

impl Store {
    pub fn path() -> PathBuf {
        data_dir().join("launcher.json")
    }

    pub fn load() -> Self {
        let path = Self::path();
        match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                eprintln!("config parse error ({e}), starting fresh");
                Store::default()
            }),
            Err(_) => Store::default(),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, text)?;
        if path.exists() {
            let _ = fs::remove_file(&path);
        }
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn active(&self) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == self.active_account)
    }

    pub fn instance(&self, id: &str) -> Option<&Instance> {
        self.instances.iter().find(|i| i.id == id)
    }
}
