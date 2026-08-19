use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{io_err, Result};
use crate::paths::{self, ensure_dir};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub ram_mb: u32,
    pub java_path: String,
    pub jvm_args: String,
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
    pub show_snapshots: bool,
    pub show_old: bool,
    pub language: String,
    pub close_on_launch: bool,
    pub accounts: Vec<Account>,
    pub active_account: String,
    pub instances: Vec<Instance>,
    pub active_instance: String,
}

impl Default for Config {
    fn default() -> Self {
        let vanilla = Instance::vanilla("Ванилла", "latest-release");
        Self {
            ram_mb: 4096,
            java_path: String::new(),
            jvm_args: "-XX:+UnlockExperimentalVMOptions -XX:+UseG1GC -XX:G1NewSizePercent=20 -XX:G1ReservePercent=20 -XX:MaxGCPauseMillis=50 -XX:G1HeapRegionSize=32M".into(),
            width: 1280,
            height: 720,
            fullscreen: false,
            show_snapshots: false,
            show_old: false,
            language: "ru".into(),
            close_on_launch: false,
            accounts: vec![Account::offline("Player")],
            active_account: String::new(),
            instances: vec![vanilla.clone()],
            active_instance: vanilla.id,
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let path = paths::config_path();
        let mut cfg = match fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
            Err(_) => Config::default(),
        };
        if cfg.instances.is_empty() {
            let vanilla = Instance::vanilla("Ванилла", "latest-release");
            cfg.active_instance = vanilla.id.clone();
            cfg.instances.push(vanilla);
        }
        if cfg.accounts.is_empty() {
            cfg.accounts.push(Account::offline("Player"));
        }
        if cfg.active_account.is_empty() {
            cfg.active_account = cfg.accounts[0].id.clone();
        }
        if !cfg.instances.iter().any(|i| i.id == cfg.active_instance) {
            cfg.active_instance = cfg.instances[0].id.clone();
        }
        if !cfg.accounts.iter().any(|a| a.id == cfg.active_account) {
            cfg.active_account = cfg.accounts[0].id.clone();
        }
        cfg.ram_mb = cfg.ram_mb.clamp(1024, 32768);
        cfg
    }

    pub fn save(&self) -> Result<()> {
        ensure_dir(&paths::data_dir())?;
        let path = paths::config_path();
        let raw = serde_json::to_string_pretty(self)?;
        fs::write(&path, raw).map_err(|source| io_err(path, source))
    }

    pub fn active_instance(&self) -> Option<&Instance> {
        self.instances.iter().find(|i| i.id == self.active_instance)
    }

    pub fn active_instance_mut(&mut self) -> Option<&mut Instance> {
        let id = self.active_instance.clone();
        self.instances.iter_mut().find(|i| i.id == id)
    }

    pub fn active_account(&self) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == self.active_account)
    }

    pub fn active_account_mut(&mut self) -> Option<&mut Account> {
        let id = self.active_account.clone();
        self.accounts.iter_mut().find(|a| a.id == id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub kind: AccountKind,
    pub name: String,
    pub uuid: String,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub xuid: String,
    #[serde(default)]
    pub expires_at: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AccountKind {
    Offline,
    Microsoft,
}

impl Account {
    pub fn offline(name: &str) -> Self {
        let name = sanitize_name(name);
        Self {
            id: Uuid::new_v4().to_string(),
            kind: AccountKind::Offline,
            uuid: offline_uuid(&name),
            name,
            access_token: "0".into(),
            refresh_token: String::new(),
            xuid: "0".into(),
            expires_at: 0,
        }
    }

    pub fn user_type(&self) -> &'static str {
        match self.kind {
            AccountKind::Offline => "legacy",
            AccountKind::Microsoft => "msa",
        }
    }

    pub fn kind_label(&self) -> &'static str {
        match self.kind {
            AccountKind::Offline => "офлайн",
            AccountKind::Microsoft => "Microsoft",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instance {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default = "vanilla_loader")]
    pub loader: String,
    #[serde(default)]
    pub fabric_loader: String,
    #[serde(default)]
    pub last_played: i64,
    #[serde(default)]
    pub ram_override: u32,
}

fn vanilla_loader() -> String {
    "vanilla".into()
}

impl Instance {
    pub fn vanilla(name: &str, version: &str) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.to_string(),
            version: version.to_string(),
            loader: "vanilla".into(),
            fabric_loader: String::new(),
            last_played: 0,
            ram_override: 0,
        }
    }

    pub fn launch_version(&self) -> String {
        if self.loader == "fabric" && !self.fabric_loader.is_empty() {
            format!("{}-fabric-{}", self.version, self.fabric_loader)
        } else {
            self.version.clone()
        }
    }

    pub fn loader_label(&self) -> String {
        if self.loader == "fabric" {
            format!("Fabric {}", self.fabric_loader)
        } else {
            "Vanilla".into()
        }
    }

    pub fn game_dir(&self) -> PathBuf {
        paths::instance_game_dir(&self.id)
    }

    pub fn ram_mb(&self, fallback: u32) -> u32 {
        if self.ram_override >= 1024 {
            self.ram_override
        } else {
            fallback
        }
    }
}

pub fn sanitize_name(name: &str) -> String {
    let trimmed = name.trim();
    let cleaned: String = trimmed
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .take(16)
        .collect();
    if cleaned.is_empty() {
        "Player".into()
    } else {
        cleaned
    }
}

/// Java `UUID.nameUUIDFromBytes("OfflinePlayer:{name}")`.
pub fn offline_uuid(name: &str) -> String {
    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(format!("OfflinePlayer:{name}").as_bytes());
    let mut bytes = hasher.finalize();
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_uuid_matches_java() {
        let uuid = offline_uuid("Steve");
        assert_eq!(uuid.len(), 36);
        assert_eq!(&uuid[14..15], "3");
    }
}
