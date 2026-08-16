//! Types mirroring the pieces of Mojang's version JSON we actually use.

use serde::Deserialize;
use std::collections::HashMap;

pub const VERSION_MANIFEST: &str =
    "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json";
pub const RESOURCES_BASE: &str = "https://resources.download.minecraft.net";

#[derive(Debug, Clone, Deserialize)]
pub struct ManifestEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    #[serde(rename = "releaseTime", default)]
    pub release_time: String,
    #[serde(default)]
    pub sha1: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VersionManifest {
    pub latest: Latest,
    pub versions: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Latest {
    pub release: String,
    pub snapshot: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Artifact {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub sha1: String,
    #[serde(default)]
    pub size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LibraryDownloads {
    #[serde(default)]
    pub artifact: Option<Artifact>,
    #[serde(default)]
    pub classifiers: HashMap<String, Artifact>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OsRule {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arch: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    pub action: String,
    #[serde(default)]
    pub os: Option<OsRule>,
    #[serde(default)]
    pub features: Option<HashMap<String, bool>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Library {
    pub name: String,
    #[serde(default)]
    pub downloads: Option<LibraryDownloads>,
    #[serde(default)]
    pub rules: Option<Vec<Rule>>,
    #[serde(default)]
    pub natives: Option<HashMap<String, String>>,
    #[serde(default)]
    pub extract: Option<Extract>,
    /// Present on Forge-style libraries that have no `downloads` block.
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Extract {
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetIndexRef {
    pub id: String,
    #[serde(default)]
    pub sha1: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub total_size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Downloads {
    #[serde(default)]
    pub client: Option<Artifact>,
    #[serde(default)]
    pub server: Option<Artifact>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JavaVersion {
    #[serde(default)]
    pub component: String,
    #[serde(rename = "majorVersion", default)]
    pub major_version: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VersionDetail {
    pub id: String,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(rename = "mainClass")]
    pub main_class: String,
    #[serde(default)]
    pub libraries: Vec<Library>,
    #[serde(default)]
    pub downloads: Downloads,
    #[serde(rename = "assetIndex", default)]
    pub asset_index: Option<AssetIndexRef>,
    #[serde(default)]
    pub assets: Option<String>,
    /// Modern (1.13+) argument format.
    #[serde(default)]
    pub arguments: Option<Arguments>,
    /// Legacy (<= 1.12) argument string.
    #[serde(rename = "minecraftArguments", default)]
    pub minecraft_arguments: Option<String>,
    #[serde(rename = "javaVersion", default)]
    pub java_version: Option<JavaVersion>,
    #[serde(rename = "inheritsFrom", default)]
    pub inherits_from: Option<String>,
    #[serde(default)]
    pub logging: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Arguments {
    #[serde(default)]
    pub game: Vec<serde_json::Value>,
    #[serde(default)]
    pub jvm: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetObject {
    pub hash: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetIndex {
    pub objects: HashMap<String, AssetObject>,
    #[serde(default)]
    pub map_to_resources: bool,
    #[serde(rename = "virtual", default)]
    pub is_virtual: bool,
}

// ------------------------------------------------------------------ rules

pub fn current_os() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "osx"
    } else {
        "linux"
    }
}

pub fn current_arch() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(target_arch = "x86") {
        "x86"
    } else {
        "unknown"
    }
}

/// Mojang uses `x86`/`x64`-ish names in `natives` keys; map ours onto theirs.
pub fn natives_key(natives: &HashMap<String, String>) -> Option<String> {
    let os = current_os();
    natives.get(os).map(|template| {
        template.replace(
            "${arch}",
            if cfg!(target_pointer_width = "64") { "64" } else { "32" },
        )
    })
}

fn os_matches(os: &OsRule) -> bool {
    if let Some(name) = &os.name {
        if name != current_os() {
            return false;
        }
    }
    if let Some(arch) = &os.arch {
        let ours = current_arch();
        let ok = match arch.as_str() {
            "x86" => ours == "x86",
            "x86_64" | "x64" => ours == "x86_64",
            "arm64" | "aarch64" => ours == "aarch64" || ours == "arm64",
            other => other == ours,
        };
        if !ok {
            return false;
        }
    }
    // `version` is a regex against the OS version; being permissive here is
    // the safe choice (worst case we download one extra native).
    true
}

/// Evaluate Mojang's allow/disallow rule list.
pub fn rules_allow(rules: &Option<Vec<Rule>>, features: &HashMap<String, bool>) -> bool {
    let Some(rules) = rules else {
        return true;
    };
    if rules.is_empty() {
        return true;
    }

    let mut allowed = false;
    for rule in rules {
        let mut applies = true;
        if let Some(os) = &rule.os {
            applies &= os_matches(os);
        }
        if let Some(required) = &rule.features {
            for (key, want) in required {
                let have = features.get(key).copied().unwrap_or(false);
                applies &= have == *want;
            }
        }
        if applies {
            allowed = rule.action == "allow";
        }
    }
    allowed
}

/// Convert a Maven coordinate (`group:artifact:version[:classifier]`) into a
/// relative path inside the libraries directory.
pub fn maven_to_path(coord: &str) -> Option<String> {
    let mut parts = coord.split(':');
    let group = parts.next()?.replace('.', "/");
    let artifact = parts.next()?;
    let version = parts.next()?;
    let classifier = parts.next();

    let (version, extension) = match version.split_once('@') {
        Some((v, ext)) => (v, ext),
        None => (version, "jar"),
    };

    let file = match classifier {
        Some(c) => format!("{artifact}-{version}-{c}.{extension}"),
        None => format!("{artifact}-{version}.{extension}"),
    };
    Some(format!("{group}/{artifact}/{version}/{file}"))
}
