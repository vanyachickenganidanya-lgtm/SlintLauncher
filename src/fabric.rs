use serde::Deserialize;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::mojang::{self, save_version_meta};

const FABRIC_META: &str = "https://meta.fabricmc.net/v2";

#[derive(Debug, Clone, Deserialize)]
pub struct LoaderVersion {
    pub loader: LoaderInfo,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoaderInfo {
    pub version: String,
    #[serde(default)]
    pub stable: bool,
}

pub async fn latest_loader(client: &reqwest::Client, game: &str) -> Result<String> {
    let url = format!("{FABRIC_META}/versions/loader/{game}");
    let list: Vec<LoaderVersion> = client.get(url).send().await?.error_for_status()?.json().await?;
    list.iter()
        .find(|l| l.loader.stable)
        .or(list.first())
        .map(|l| l.loader.version.clone())
        .ok_or_else(|| Error::msg(format!("Fabric не поддерживает {game}")))
}

pub async fn install_profile(
    client: &reqwest::Client,
    game: &str,
    loader: &str,
) -> Result<String> {
    let url = format!("{FABRIC_META}/versions/loader/{game}/{loader}/profile/json");
    let profile: Value = client.get(url).send().await?.error_for_status()?.json().await?;
    let id = profile
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if id.is_empty() {
        return Err(Error::msg("Fabric вернул пустой профиль"));
    }
    save_version_meta(&id, &profile)?;
    // Touch-cache the inherited vanilla metadata if we already know it.
    let _ = id;
    Ok(id)
}

pub async fn ensure_fabric(
    client: &reqwest::Client,
    manifest: &mojang::VersionManifest,
    game: &str,
    loader: &str,
) -> Result<mojang::VersionMeta> {
    let id = install_profile(client, game, loader).await?;
    mojang::resolve_version(client, manifest, &id).await
}
