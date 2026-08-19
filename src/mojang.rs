use std::collections::HashMap;
use std::fs;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{io_err, Error, Result};
use crate::paths::{self, ensure_dir, version_json};
use crate::rules::{parse_rules, rules_allow, Features, Rule};

pub const MANIFEST_URL: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
pub const NEWS_URL: &str = "https://launchercontent.mojang.com/javaPatchNotes.json";
pub const USER_AGENT: &str = concat!("SlintLauncher/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VersionManifest {
    pub latest: LatestVersions,
    pub versions: Vec<VersionInfo>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LatestVersions {
    pub release: String,
    pub snapshot: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VersionInfo {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    #[serde(rename = "releaseTime", default)]
    pub release_time: String,
    #[serde(default)]
    pub sha1: String,
}

impl VersionInfo {
    pub fn kind_label(&self) -> &str {
        match self.kind.as_str() {
            "release" => "релиз",
            "snapshot" => "снимок",
            "old_beta" => "бета",
            "old_alpha" => "альфа",
            other => other,
        }
    }

    pub fn date_short(&self) -> String {
        self.release_time.get(..10).unwrap_or(&self.release_time).to_string()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct VersionMeta {
    pub id: String,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(rename = "mainClass", default)]
    pub main_class: String,
    #[serde(default)]
    pub arguments: Option<Arguments>,
    #[serde(rename = "minecraftArguments", default)]
    pub minecraft_arguments: Option<String>,
    #[serde(default)]
    pub libraries: Vec<Library>,
    pub downloads: Option<Downloads>,
    #[serde(rename = "assetIndex")]
    pub asset_index: Option<AssetIndexInfo>,
    #[serde(default)]
    pub assets: Option<String>,
    #[serde(rename = "javaVersion")]
    pub java_version: Option<JavaVersion>,
    #[serde(rename = "inheritsFrom")]
    pub inherits_from: Option<String>,
    #[serde(default)]
    pub logging: Option<Logging>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Arguments {
    #[serde(default)]
    pub game: Vec<Value>,
    #[serde(default)]
    pub jvm: Vec<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Library {
    pub name: String,
    #[serde(default)]
    pub downloads: Option<LibraryDownloads>,
    #[serde(default)]
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub natives: Option<HashMap<String, String>>,
    #[serde(default)]
    pub extract: Option<Extract>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LibraryDownloads {
    #[serde(default)]
    pub artifact: Option<Artifact>,
    #[serde(default)]
    pub classifiers: Option<HashMap<String, Artifact>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Artifact {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Extract {
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Downloads {
    pub client: Option<Artifact>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetIndexInfo {
    pub id: String,
    pub sha1: String,
    pub size: u64,
    #[serde(rename = "totalSize", default)]
    pub total_size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JavaVersion {
    #[serde(default)]
    pub component: String,
    #[serde(rename = "majorVersion", default)]
    pub major_version: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Logging {
    pub client: Option<LoggingClient>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoggingClient {
    pub argument: String,
    pub file: Artifact,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetIndex {
    #[serde(default)]
    pub objects: HashMap<String, AssetObject>,
    #[serde(default)]
    pub map_to_resources: bool,
    #[serde(default)]
    pub virtual_dir: bool,
    #[serde(rename = "virtual", default)]
    pub virtual_flag: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetObject {
    pub hash: String,
    pub size: u64,
}

pub fn http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(60))
        .build()?)
}

pub async fn fetch_manifest(client: &reqwest::Client) -> Result<VersionManifest> {
    ensure_dir(&paths::minecraft_dir())?;
    let cache = paths::minecraft_dir().join("version_manifest_v2.json");
    let response = client.get(MANIFEST_URL).send().await?;
    if !response.status().is_success() {
        if cache.exists() {
            let raw = fs::read_to_string(&cache).map_err(|s| io_err(&cache, s))?;
            return Ok(serde_json::from_str(&raw)?);
        }
        return Err(Error::msg(format!(
            "манифест версий: HTTP {}",
            response.status()
        )));
    }
    let body = response.text().await?;
    fs::write(&cache, &body).map_err(|s| io_err(&cache, s))?;
    Ok(serde_json::from_str(&body)?)
}

pub async fn fetch_version_meta(
    client: &reqwest::Client,
    info: &VersionInfo,
) -> Result<VersionMeta> {
    let path = version_json(&info.id);
    ensure_dir(path.parent().unwrap())?;
    if path.exists() && !info.sha1.is_empty() {
        if let Ok(raw) = fs::read(&path) {
            if crate::install::sha1_hex(&raw) == info.sha1 {
                return parse_version_meta(&raw);
            }
        }
    }
    let bytes = client.get(&info.url).send().await?.bytes().await?;
    if !info.sha1.is_empty() && crate::install::sha1_hex(&bytes) != info.sha1 {
        return Err(Error::Checksum { path });
    }
    fs::write(&path, &bytes).map_err(|s| io_err(&path, s))?;
    parse_version_meta(&bytes)
}

pub fn load_local_version(id: &str) -> Result<VersionMeta> {
    let path = version_json(id);
    let raw = fs::read(&path).map_err(|s| io_err(&path, s))?;
    parse_version_meta(&raw)
}

fn parse_version_meta(raw: &[u8]) -> Result<VersionMeta> {
    Ok(serde_json::from_slice(raw)?)
}

pub async fn resolve_version(
    client: &reqwest::Client,
    manifest: &VersionManifest,
    id: &str,
) -> Result<VersionMeta> {
    let resolved = match id {
        "latest-release" => manifest.latest.release.as_str(),
        "latest-snapshot" => manifest.latest.snapshot.as_str(),
        other => other,
    };
    if let Ok(local) = load_local_version(resolved) {
        return merge_inherits(client, manifest, local).await;
    }
    let info = manifest
        .versions
        .iter()
        .find(|v| v.id == resolved)
        .ok_or_else(|| Error::VersionNotFound(resolved.to_string()))?;
    let meta = fetch_version_meta(client, info).await?;
    merge_inherits(client, manifest, meta).await
}

async fn merge_inherits(
    client: &reqwest::Client,
    manifest: &VersionManifest,
    child: VersionMeta,
) -> Result<VersionMeta> {
    let Some(parent_id) = child.inherits_from.clone() else {
        return Ok(child);
    };
    let parent = Box::pin(resolve_version(client, manifest, &parent_id)).await?;
    Ok(merge_meta(parent, child))
}

fn merge_meta(mut parent: VersionMeta, child: VersionMeta) -> VersionMeta {
    parent.id = child.id;
    if !child.main_class.is_empty() {
        parent.main_class = child.main_class;
    }
    if child.arguments.is_some() {
        match (&mut parent.arguments, child.arguments) {
            (Some(dst), Some(src)) => {
                dst.game.extend(src.game);
                dst.jvm.extend(src.jvm);
            }
            (dst, Some(src)) => *dst = Some(src),
            _ => {}
        }
    }
    if child.minecraft_arguments.is_some() {
        parent.minecraft_arguments = child.minecraft_arguments;
    }
    parent.libraries.extend(child.libraries);
    if child.downloads.is_some() {
        parent.downloads = child.downloads;
    }
    if child.asset_index.is_some() {
        parent.asset_index = child.asset_index;
    }
    if child.java_version.is_some() {
        parent.java_version = child.java_version;
    }
    if child.logging.is_some() {
        parent.logging = child.logging;
    }
    parent.inherits_from = None;
    parent
}

pub fn save_version_meta(id: &str, value: &Value) -> Result<()> {
    let path = version_json(id);
    ensure_dir(path.parent().unwrap())?;
    let raw = serde_json::to_vec_pretty(value)?;
    fs::write(&path, raw).map_err(|s| io_err(&path, s))
}

pub fn library_allowed(lib: &Library, features: &Features) -> bool {
    rules_allow(&lib.rules, features)
}

pub fn native_classifier(lib: &Library) -> Option<String> {
    lib.natives.as_ref()?.get(crate::rules::current_os()).cloned()
}

pub fn artifact_path(name: &str, classifier: Option<&str>) -> String {
    let parts: Vec<&str> = name.split(':').collect();
    if parts.len() < 3 {
        return format!("{name}.jar");
    }
    let group = parts[0].replace('.', "/");
    let artifact = parts[1];
    let version = parts[2];
    let classifier = classifier.or(parts.get(3).copied());
    match classifier {
        Some(c) if !c.is_empty() => {
            format!("{group}/{artifact}/{version}/{artifact}-{version}-{c}.jar")
        }
        _ => format!("{group}/{artifact}/{version}/{artifact}-{version}.jar"),
    }
}

pub fn argument_strings(items: &[Value], features: &Features) -> Vec<String> {
    let mut out = Vec::new();
    for item in items {
        match item {
            Value::String(s) => out.push(s.clone()),
            Value::Object(map) => {
                if argument_allowed(map, features) {
                    match map.get("value") {
                        Some(Value::String(s)) => out.push(s.clone()),
                        Some(Value::Array(arr)) => {
                            for v in arr {
                                if let Some(s) = v.as_str() {
                                    out.push(s.to_string());
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn argument_allowed(map: &Map<String, Value>, features: &Features) -> bool {
    let rules = parse_rules(&Value::Object(map.clone()));
    rules_allow(&rules, features)
}
