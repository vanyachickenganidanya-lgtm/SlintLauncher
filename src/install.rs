use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use futures::stream::{self, StreamExt};
use sha1::{Digest, Sha1};

use crate::error::{io_err, Error, Result};
use crate::mojang::{
    self, artifact_path, library_allowed, native_classifier, Artifact, Library, VersionMeta,
};
use crate::paths::{self, ensure_dir};
use crate::rules::Features;

pub const ASSET_CDN: &str = "https://resources.download.minecraft.net";
const LIB_CDN: &str = "https://libraries.minecraft.net";

#[derive(Clone)]
pub struct Progress {
    pub stage: String,
    pub current: u64,
    pub total: u64,
    pub detail: String,
}

pub type ProgressFn = Arc<dyn Fn(Progress) + Send + Sync>;

pub async fn install_version(
    client: &reqwest::Client,
    meta: &VersionMeta,
    on_progress: ProgressFn,
) -> Result<()> {
    ensure_dir(&paths::version_dir(&meta.id))?;
    ensure_dir(&paths::libraries_dir())?;
    ensure_dir(&paths::assets_dir())?;

    download_client(client, meta, &on_progress).await?;
    download_libraries(client, meta, &on_progress).await?;
    download_assets(client, meta, &on_progress).await?;
    if let Some(logging) = meta.logging.as_ref().and_then(|l| l.client.as_ref()) {
        download_logging(client, logging).await?;
    }
    Ok(())
}

async fn download_client(
    client: &reqwest::Client,
    meta: &VersionMeta,
    on_progress: &ProgressFn,
) -> Result<()> {
    let Some(artifact) = meta.downloads.as_ref().and_then(|d| d.client.as_ref()) else {
        return Ok(());
    };
    let dest = paths::version_jar(&meta.id);
    on_progress(Progress {
        stage: "Клиент".into(),
        current: 0,
        total: 1,
        detail: dest
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into(),
    });
    fetch_artifact(client, artifact, &dest, None).await?;
    on_progress(Progress {
        stage: "Клиент".into(),
        current: 1,
        total: 1,
        detail: "готово".into(),
    });
    Ok(())
}

async fn download_libraries(
    client: &reqwest::Client,
    meta: &VersionMeta,
    on_progress: &ProgressFn,
) -> Result<()> {
    let features = Features::default();
    let mut jobs = Vec::new();
    for lib in &meta.libraries {
        if !library_allowed(lib, &features) {
            continue;
        }
        if let Some(job) = library_artifact_job(lib) {
            jobs.push(job);
        }
        if let Some(job) = native_artifact_job(lib) {
            jobs.push(job);
        }
    }
    download_jobs(client, "Библиотеки", jobs, on_progress).await
}

fn library_artifact_job(lib: &Library) -> Option<DownloadJob> {
    if let Some(art) = lib.downloads.as_ref().and_then(|d| d.artifact.as_ref()) {
        let rel = art
            .path
            .clone()
            .unwrap_or_else(|| artifact_path(&lib.name, None));
        let dest = paths::libraries_dir().join(&rel);
        return Some(DownloadJob {
            dest,
            url: art.url.clone().unwrap_or_else(|| format!("{LIB_CDN}/{rel}")),
            sha1: art.sha1.clone().unwrap_or_default(),
            size: art.size.unwrap_or(0),
        });
    }
    if lib.natives.is_some() && lib.downloads.is_some() {
        return None;
    }
    let rel = artifact_path(&lib.name, None);
    let base = lib
        .url
        .clone()
        .unwrap_or_else(|| format!("{LIB_CDN}/"));
    let url = if base.ends_with('/') {
        format!("{base}{rel}")
    } else {
        format!("{base}/{rel}")
    };
    Some(DownloadJob {
        dest: paths::libraries_dir().join(&rel),
        url,
        sha1: String::new(),
        size: 0,
    })
}

fn native_artifact_job(lib: &Library) -> Option<DownloadJob> {
    let classifier = native_classifier(lib)?;
    let classifier = classifier.replace("${arch}", native_arch_token());
    if let Some(art) = lib
        .downloads
        .as_ref()
        .and_then(|d| d.classifiers.as_ref())
        .and_then(|c| c.get(&classifier))
    {
        let rel = art
            .path
            .clone()
            .unwrap_or_else(|| artifact_path(&lib.name, Some(&classifier)));
        return Some(DownloadJob {
            dest: paths::libraries_dir().join(&rel),
            url: art.url.clone().unwrap_or_else(|| format!("{LIB_CDN}/{rel}")),
            sha1: art.sha1.clone().unwrap_or_default(),
            size: art.size.unwrap_or(0),
        });
    }
    let rel = artifact_path(&lib.name, Some(&classifier));
    Some(DownloadJob {
        dest: paths::libraries_dir().join(&rel),
        url: format!("{LIB_CDN}/{rel}"),
        sha1: String::new(),
        size: 0,
    })
}

fn native_arch_token() -> &'static str {
    if cfg!(target_arch = "x86") {
        "32"
    } else {
        "64"
    }
}

async fn download_assets(
    client: &reqwest::Client,
    meta: &VersionMeta,
    on_progress: &ProgressFn,
) -> Result<()> {
    let Some(index_info) = &meta.asset_index else {
        return Ok(());
    };
    let index_path = paths::assets_dir()
        .join("indexes")
        .join(format!("{}.json", index_info.id));
    ensure_dir(index_path.parent().unwrap())?;
    fetch_artifact(
        client,
        &Artifact {
            path: None,
            sha1: Some(index_info.sha1.clone()),
            size: Some(index_info.size),
            url: Some(index_info.url.clone()),
        },
        &index_path,
        None,
    )
    .await?;
    let raw = fs::read(&index_path).map_err(|s| io_err(&index_path, s))?;
    let index: mojang::AssetIndex = serde_json::from_slice(&raw)?;
    let mut jobs = Vec::new();
    for (_name, obj) in index.objects {
        let prefix = &obj.hash[..2];
        let rel = format!("{prefix}/{}", obj.hash);
        jobs.push(DownloadJob {
            dest: paths::assets_dir().join("objects").join(&rel),
            url: format!("{ASSET_CDN}/{rel}"),
            sha1: obj.hash,
            size: obj.size,
        });
    }
    download_jobs(client, "Ресурсы", jobs, on_progress).await
}

async fn download_logging(
    client: &reqwest::Client,
    logging: &mojang::LoggingClient,
) -> Result<()> {
    let name = logging
        .file
        .path
        .clone()
        .or_else(|| logging.file.url.as_ref().and_then(|u| {
            u.rsplit('/').next().map(|s| s.to_string())
        }))
        .unwrap_or_else(|| "client.xml".into());
    let dest = paths::assets_dir().join("log_configs").join(name);
    fetch_artifact(client, &logging.file, &dest, None).await
}

#[derive(Clone)]
struct DownloadJob {
    dest: PathBuf,
    url: String,
    sha1: String,
    size: u64,
}

async fn download_jobs(
    client: &reqwest::Client,
    stage: &str,
    jobs: Vec<DownloadJob>,
    on_progress: &ProgressFn,
) -> Result<()> {
    let total = jobs.len() as u64;
    let done = Arc::new(AtomicU64::new(0));
    on_progress(Progress {
        stage: stage.into(),
        current: 0,
        total,
        detail: format!("0 / {total}"),
    });

    let results: Vec<Result<()>> = stream::iter(jobs.into_iter())
        .map(|job| {
            let client = client.clone();
            let done = done.clone();
            let on_progress = on_progress.clone();
            let stage = stage.to_string();
            async move {
                fetch_artifact(&client, &job_as_artifact(&job), &job.dest, Some(job.size)).await?;
                let current = done.fetch_add(1, Ordering::Relaxed) + 1;
                if current == total || current % 8 == 0 {
                    on_progress(Progress {
                        stage,
                        current,
                        total,
                        detail: format!("{current} / {total}"),
                    });
                }
                Ok(())
            }
        })
        .buffer_unordered(24)
        .collect()
        .await;

    for result in results {
        result?;
    }
    on_progress(Progress {
        stage: stage.into(),
        current: total,
        total,
        detail: "готово".into(),
    });
    Ok(())
}

fn job_as_artifact(job: &DownloadJob) -> Artifact {
    Artifact {
        path: None,
        sha1: if job.sha1.is_empty() {
            None
        } else {
            Some(job.sha1.clone())
        },
        size: if job.size == 0 { None } else { Some(job.size) },
        url: Some(job.url.clone()),
    }
}

async fn fetch_artifact(
    client: &reqwest::Client,
    artifact: &Artifact,
    dest: &Path,
    expected_size: Option<u64>,
) -> Result<()> {
    if dest.exists() {
        if file_ok(dest, artifact.sha1.as_deref(), expected_size.or(artifact.size))? {
            return Ok(());
        }
    }
    let url = artifact
        .url
        .as_deref()
        .ok_or_else(|| Error::msg(format!("нет URL для {}", dest.display())))?;
    if let Some(parent) = dest.parent() {
        ensure_dir(parent)?;
    }
    let tmp = dest.with_extension("part");
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        return Err(Error::msg(format!(
            "загрузка {} → HTTP {}",
            dest.display(),
            response.status()
        )));
    }
    let bytes = response.bytes().await?;
    if let Some(sum) = artifact.sha1.as_deref() {
        if !sum.is_empty() && sha1_hex(&bytes) != sum {
            return Err(Error::Checksum {
                path: dest.to_path_buf(),
            });
        }
    }
    fs::write(&tmp, &bytes).map_err(|s| io_err(&tmp, s))?;
    fs::rename(&tmp, dest).map_err(|s| io_err(dest, s))?;
    Ok(())
}

fn file_ok(path: &Path, sha1: Option<&str>, size: Option<u64>) -> Result<bool> {
    let meta = fs::metadata(path).map_err(|s| io_err(path, s))?;
    if let Some(expected) = size {
        if expected > 0 && meta.len() != expected {
            return Ok(false);
        }
    }
    if let Some(sum) = sha1 {
        if !sum.is_empty() {
            let data = fs::read(path).map_err(|s| io_err(path, s))?;
            return Ok(sha1_hex(&data) == sum);
        }
    }
    Ok(meta.len() > 0)
}

pub fn sha1_hex(data: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

pub fn extract_natives(meta: &VersionMeta) -> Result<PathBuf> {
    let dest = paths::natives_dir(&meta.id);
    if dest.exists() {
        let _ = fs::remove_dir_all(&dest);
    }
    ensure_dir(&dest)?;
    let features = Features::default();
    for lib in &meta.libraries {
        if !library_allowed(lib, &features) {
            continue;
        }
        let Some(classifier) = native_classifier(lib) else {
            continue;
        };
        let classifier = classifier.replace("${arch}", native_arch_token());
        let rel = lib
            .downloads
            .as_ref()
            .and_then(|d| d.classifiers.as_ref())
            .and_then(|c| c.get(&classifier))
            .and_then(|a| a.path.clone())
            .unwrap_or_else(|| artifact_path(&lib.name, Some(&classifier)));
        let jar = paths::libraries_dir().join(rel);
        if !jar.exists() {
            continue;
        }
        let exclude = lib
            .extract
            .as_ref()
            .map(|e| e.exclude.clone())
            .unwrap_or_default();
        unzip_natives(&jar, &dest, &exclude)?;
    }
    Ok(dest)
}

fn unzip_natives(jar: &Path, dest: &Path, exclude: &[String]) -> Result<()> {
    let file = File::open(jar).map_err(|s| io_err(jar, s))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| Error::msg(e.to_string()))?;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| Error::msg(e.to_string()))?;
        let name = entry.name().replace('\\', "/");
        if name.ends_with('/') || name.starts_with("META-INF") {
            continue;
        }
        if exclude.iter().any(|ex| name.starts_with(ex)) {
            continue;
        }
        let out_path = dest.join(Path::new(&name).file_name().unwrap_or_default());
        let mut out = File::create(&out_path).map_err(|s| io_err(&out_path, s))?;
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).map_err(|s| io_err(&out_path, s))?;
        out.write_all(&buf).map_err(|s| io_err(&out_path, s))?;
    }
    Ok(())
}

pub fn classpath(meta: &VersionMeta) -> Result<String> {
    let features = Features::default();
    let mut entries = Vec::new();
    for lib in &meta.libraries {
        if !library_allowed(lib, &features) {
            continue;
        }
        if lib.natives.is_some() && lib.downloads.as_ref().and_then(|d| d.artifact.as_ref()).is_none()
        {
            continue;
        }
        if let Some(art) = lib.downloads.as_ref().and_then(|d| d.artifact.as_ref()) {
            let rel = art
                .path
                .clone()
                .unwrap_or_else(|| artifact_path(&lib.name, None));
            entries.push(paths::libraries_dir().join(rel));
        } else if lib.natives.is_none() {
            entries.push(paths::libraries_dir().join(artifact_path(&lib.name, None)));
        }
    }
    let jar = paths::version_jar(&meta.id);
    if jar.exists() {
        entries.push(jar);
    } else if let Some(parent) = meta.inherits_from.as_ref() {
        entries.push(paths::version_jar(parent));
    }
    Ok(entries
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(paths::classpath_sep()))
}
