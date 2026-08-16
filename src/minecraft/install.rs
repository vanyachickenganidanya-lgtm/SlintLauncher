//! Downloading a Minecraft version: client jar, libraries, natives, assets.

use super::manifest::*;
use crate::util::{agent, download, ensure_parent, file_ok, get_json};
use anyhow::{anyhow, bail, Context, Result};
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Progress reporting back to the UI thread.
pub type Reporter = Arc<dyn Fn(&str, f32) + Send + Sync>;

pub struct Paths {
    pub root: PathBuf,
}

impl Paths {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
    pub fn versions(&self) -> PathBuf {
        self.root.join("versions")
    }
    pub fn version_dir(&self, id: &str) -> PathBuf {
        self.versions().join(id)
    }
    pub fn version_json(&self, id: &str) -> PathBuf {
        self.version_dir(id).join(format!("{id}.json"))
    }
    pub fn version_jar(&self, id: &str) -> PathBuf {
        self.version_dir(id).join(format!("{id}.jar"))
    }
    pub fn libraries(&self) -> PathBuf {
        self.root.join("libraries")
    }
    pub fn natives(&self, id: &str) -> PathBuf {
        self.version_dir(id).join("natives")
    }
    pub fn assets(&self) -> PathBuf {
        self.root.join("assets")
    }
    pub fn asset_indexes(&self) -> PathBuf {
        self.assets().join("indexes")
    }
    pub fn asset_objects(&self) -> PathBuf {
        self.assets().join("objects")
    }
    pub fn instances(&self) -> PathBuf {
        self.root.join("instances")
    }
}

pub fn fetch_manifest() -> Result<VersionManifest> {
    let json = get_json(&agent(), VERSION_MANIFEST)?;
    Ok(serde_json::from_value(json)?)
}

/// Load the version JSON, downloading it when missing, and merge any
/// `inheritsFrom` parent (used by modded/legacy profiles).
pub fn load_version(paths: &Paths, entry: Option<&ManifestEntry>, id: &str) -> Result<VersionDetail> {
    let json_path = paths.version_json(id);

    if !json_path.exists() {
        let entry = match entry {
            Some(e) => e.clone(),
            None => fetch_manifest()?
                .versions
                .into_iter()
                .find(|v| v.id == id)
                .ok_or_else(|| anyhow!("version {id} not found in Mojang manifest"))?,
        };
        ensure_parent(&json_path)?;
        download(&agent(), &entry.url, &json_path, None, None)?;
    }

    let text = fs::read_to_string(&json_path)
        .with_context(|| format!("reading {}", json_path.display()))?;
    let mut detail: VersionDetail = serde_json::from_str(&text)
        .with_context(|| format!("parsing {}", json_path.display()))?;

    if let Some(parent_id) = detail.inherits_from.clone() {
        let parent = load_version(paths, None, &parent_id)?;
        detail = merge_versions(parent, detail);
    }
    Ok(detail)
}

fn merge_versions(parent: VersionDetail, child: VersionDetail) -> VersionDetail {
    let mut libraries = child.libraries;
    libraries.extend(parent.libraries);

    let arguments = match (child.arguments, parent.arguments) {
        (Some(c), Some(p)) => Some(Arguments {
            game: [p.game, c.game].concat(),
            jvm: [p.jvm, c.jvm].concat(),
        }),
        (Some(c), None) => Some(c),
        (None, p) => p,
    };

    VersionDetail {
        id: child.id,
        kind: if child.kind.is_empty() { parent.kind } else { child.kind },
        main_class: child.main_class,
        libraries,
        downloads: if child.downloads.client.is_some() { child.downloads } else { parent.downloads },
        asset_index: child.asset_index.or(parent.asset_index),
        assets: child.assets.or(parent.assets),
        arguments,
        minecraft_arguments: child.minecraft_arguments.or(parent.minecraft_arguments),
        java_version: child.java_version.or(parent.java_version),
        inherits_from: None,
        logging: child.logging.or(parent.logging),
    }
}

pub struct ResolvedLibs {
    pub classpath: Vec<PathBuf>,
    pub natives: Vec<(PathBuf, Vec<String>)>,
}

/// Work out which libraries apply to this OS and where they live on disk.
pub fn resolve_libraries(paths: &Paths, detail: &VersionDetail) -> ResolvedLibs {
    let features: HashMap<String, bool> = HashMap::new();
    let mut classpath = Vec::new();
    let mut natives = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for lib in &detail.libraries {
        if !rules_allow(&lib.rules, &features) {
            continue;
        }

        // native classifier for this OS, if any
        if let Some(native_map) = &lib.natives {
            if let Some(key) = natives_key(native_map) {
                if let Some(downloads) = &lib.downloads {
                    if let Some(artifact) = downloads.classifiers.get(&key) {
                        if let Some(rel) = artifact.path.clone().or_else(|| maven_to_path(&format!("{}:{}", lib.name, key))) {
                            let exclude = lib.extract.clone().unwrap_or_default().exclude;
                            natives.push((paths.libraries().join(rel), exclude));
                        }
                    }
                }
                // A native-only library must not land on the classpath.
                continue;
            }
        }

        let rel = lib
            .downloads
            .as_ref()
            .and_then(|d| d.artifact.as_ref())
            .and_then(|a| a.path.clone())
            .or_else(|| maven_to_path(&lib.name));

        if let Some(rel) = rel {
            if seen.insert(rel.clone()) {
                classpath.push(paths.libraries().join(rel));
            }
        }
    }

    ResolvedLibs { classpath, natives }
}

struct Job {
    url: String,
    dest: PathBuf,
    sha1: Option<String>,
    size: Option<u64>,
}

/// Download everything the version needs. Reports progress 0.0..1.0.
pub fn install_version(paths: &Paths, detail: &VersionDetail, report: &Reporter) -> Result<()> {
    let mut jobs: Vec<Job> = Vec::new();

    // client jar
    let client = detail
        .downloads
        .client
        .as_ref()
        .ok_or_else(|| anyhow!("version {} has no client download", detail.id))?;
    jobs.push(Job {
        url: client.url.clone(),
        dest: paths.version_jar(&detail.id),
        sha1: Some(client.sha1.clone()),
        size: Some(client.size),
    });

    // libraries + natives
    let features: HashMap<String, bool> = HashMap::new();
    for lib in &detail.libraries {
        if !rules_allow(&lib.rules, &features) {
            continue;
        }

        if let Some(downloads) = &lib.downloads {
            if let Some(artifact) = &downloads.artifact {
                if let Some(rel) = artifact.path.clone().or_else(|| maven_to_path(&lib.name)) {
                    jobs.push(Job {
                        url: artifact.url.clone(),
                        dest: paths.libraries().join(rel),
                        sha1: Some(artifact.sha1.clone()),
                        size: Some(artifact.size),
                    });
                }
            }
            if let Some(native_map) = &lib.natives {
                if let Some(key) = natives_key(native_map) {
                    if let Some(artifact) = downloads.classifiers.get(&key) {
                        if let Some(rel) = artifact.path.clone() {
                            jobs.push(Job {
                                url: artifact.url.clone(),
                                dest: paths.libraries().join(rel),
                                sha1: Some(artifact.sha1.clone()),
                                size: Some(artifact.size),
                            });
                        }
                    }
                }
            }
        } else if let (Some(base), Some(rel)) = (lib.url.as_ref(), maven_to_path(&lib.name)) {
            // Legacy/modded style: base URL + maven path.
            jobs.push(Job {
                url: format!("{}/{}", base.trim_end_matches('/'), rel),
                dest: paths.libraries().join(&rel),
                sha1: None,
                size: None,
            });
        }
    }

    // asset index + objects
    let mut asset_jobs = Vec::new();
    let mut asset_index_data: Option<(String, AssetIndex)> = None;
    if let Some(index_ref) = &detail.asset_index {
        let index_path = paths.asset_indexes().join(format!("{}.json", index_ref.id));
        report("Скачивание индекса ресурсов / Fetching asset index", 0.02);
        download(
            &agent(),
            &index_ref.url,
            &index_path,
            if index_ref.sha1.is_empty() { None } else { Some(&index_ref.sha1) },
            None,
        )?;

        let index: AssetIndex = serde_json::from_str(&fs::read_to_string(&index_path)?)
            .context("parsing asset index")?;

        for object in index.objects.values() {
            let prefix = &object.hash[0..2];
            asset_jobs.push(Job {
                url: format!("{RESOURCES_BASE}/{prefix}/{}", object.hash),
                dest: paths.asset_objects().join(prefix).join(&object.hash),
                sha1: Some(object.hash.clone()),
                size: Some(object.size),
            });
        }
        asset_index_data = Some((index_ref.id.clone(), index));
    }

    jobs.extend(asset_jobs);

    // Skip everything already on disk before reporting totals.
    let pending: Vec<Job> = jobs
        .into_iter()
        .filter(|j| !file_ok(&j.dest, j.sha1.as_deref(), j.size))
        .collect();

    let total = pending.len();
    if total > 0 {
        report(&format!("Загрузка файлов: 0/{total}"), 0.0);
        let done = AtomicUsize::new(0);
        let agent = agent();

        let results: Vec<Result<()>> = pending
            .par_iter()
            .map(|job| {
                let outcome = download(&agent, &job.url, &job.dest, job.sha1.as_deref(), job.size)
                    .map(|_| ())
                    .with_context(|| format!("downloading {}", job.url));
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                if n % 8 == 0 || n == total {
                    report(
                        &format!("Загрузка файлов / Downloading: {n}/{total}"),
                        n as f32 / total as f32,
                    );
                }
                outcome
            })
            .collect();

        let errors: Vec<String> = results
            .into_iter()
            .filter_map(|r| r.err().map(|e| format!("{e:#}")))
            .collect();
        if !errors.is_empty() {
            bail!("{} file(s) failed to download:\n{}", errors.len(), errors.join("\n"));
        }
    }

    // Legacy versions read assets from a flat directory instead of the hashed store.
    if let Some((index_id, index)) = asset_index_data {
        if index.is_virtual || index.map_to_resources {
            report("Подготовка ресурсов / Preparing legacy assets", 0.97);
            let target = if index.map_to_resources {
                paths.root.join("resources")
            } else {
                paths.assets().join("virtual").join(&index_id)
            };
            for (name, object) in &index.objects {
                let src = paths.asset_objects().join(&object.hash[0..2]).join(&object.hash);
                let dst = target.join(name);
                if dst.exists() {
                    continue;
                }
                ensure_parent(&dst)?;
                let _ = fs::copy(&src, &dst);
            }
        }
    }

    extract_natives(paths, detail)?;
    report("Готово / Done", 1.0);
    Ok(())
}

/// Unpack native jars into `versions/<id>/natives`.
pub fn extract_natives(paths: &Paths, detail: &VersionDetail) -> Result<()> {
    let resolved = resolve_libraries(paths, detail);
    if resolved.natives.is_empty() {
        return Ok(());
    }
    let out_dir = paths.natives(&detail.id);
    fs::create_dir_all(&out_dir)?;

    for (jar_path, exclude) in resolved.natives {
        if !jar_path.exists() {
            continue;
        }
        let file = fs::File::open(&jar_path)?;
        let mut archive = match zip::ZipArchive::new(file) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("cannot open native jar {}: {e}", jar_path.display());
                continue;
            }
        };

        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let Some(enclosed) = entry.enclosed_name() else {
                continue;
            };
            let name = enclosed.to_string_lossy().replace('\\', "/");

            if name.ends_with('/')
                || name.starts_with("META-INF/")
                || exclude.iter().any(|p| name.starts_with(p.trim_end_matches('/')))
            {
                continue;
            }
            // Only actual native binaries matter.
            let is_native = name.ends_with(".dll")
                || name.ends_with(".so")
                || name.contains(".so.")
                || name.ends_with(".dylib")
                || name.ends_with(".jnilib");
            if !is_native {
                continue;
            }

            let file_name = Path::new(&name).file_name().unwrap_or_default();
            let dest = out_dir.join(file_name);
            if dest.exists() {
                continue;
            }
            let mut out = fs::File::create(&dest)?;
            std::io::copy(&mut entry, &mut out)?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(0o755));
            }
        }
    }
    Ok(())
}
