use anyhow::{anyhow, Context, Result};
use sha1::{Digest, Sha1};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(120))
        .user_agent(concat!("SlintLauncher/", env!("CARGO_PKG_VERSION")))
        .build()
}

pub fn sha1_file(path: &Path) -> Result<String> {
    let mut f = fs::File::open(path)?;
    let mut hasher = Sha1::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// True when the file exists and (if a hash was given) matches it.
pub fn file_ok(path: &Path, sha1: Option<&str>, size: Option<u64>) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    if let Some(size) = size {
        if meta.len() != size {
            return false;
        }
    }
    match sha1 {
        Some(expected) if !expected.is_empty() => {
            matches!(sha1_file(path), Ok(actual) if actual.eq_ignore_ascii_case(expected))
        }
        _ => true,
    }
}

pub fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create directory {}", parent.display()))?;
    }
    Ok(())
}

/// Download `url` into `dest`, verifying the SHA-1 when provided.
/// Existing valid files are left alone.
pub fn download(agent: &ureq::Agent, url: &str, dest: &Path, sha1: Option<&str>, size: Option<u64>) -> Result<bool> {
    if file_ok(dest, sha1, size) {
        return Ok(false);
    }
    ensure_parent(dest)?;

    let mut last_err = None;
    for attempt in 0..3 {
        match try_download(agent, url, dest, sha1) {
            Ok(()) => return Ok(true),
            Err(e) => {
                last_err = Some(e);
                std::thread::sleep(Duration::from_millis(400 * (attempt + 1)));
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("download failed: {url}")))
}

fn try_download(agent: &ureq::Agent, url: &str, dest: &Path, sha1: Option<&str>) -> Result<()> {
    let resp = agent
        .get(url)
        .call()
        .with_context(|| format!("GET {url}"))?;

    let tmp = dest.with_extension(format!(
        "{}.part",
        dest.extension().and_then(|e| e.to_str()).unwrap_or("tmp")
    ));

    {
        let mut reader = resp.into_reader();
        let mut file = fs::File::create(&tmp)?;
        let mut buf = vec![0u8; 128 * 1024];
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])?;
        }
        file.flush()?;
    }

    if let Some(expected) = sha1 {
        if !expected.is_empty() {
            let actual = sha1_file(&tmp)?;
            if !actual.eq_ignore_ascii_case(expected) {
                let _ = fs::remove_file(&tmp);
                return Err(anyhow!("checksum mismatch for {url}: {actual} != {expected}"));
            }
        }
    }

    if dest.exists() {
        let _ = fs::remove_file(dest);
    }
    fs::rename(&tmp, dest)?;
    Ok(())
}

pub fn get_json(agent: &ureq::Agent, url: &str) -> Result<serde_json::Value> {
    let value: serde_json::Value = agent
        .get(url)
        .call()
        .with_context(|| format!("GET {url}"))?
        .into_json()
        .with_context(|| format!("invalid JSON from {url}"))?;
    Ok(value)
}

/// Open a URL or a local folder with the system handler.
pub fn open_path(target: &str) {
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("cmd")
        .args(["/C", "start", "", target])
        .spawn();

    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(target).spawn();

    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(target).spawn();

    if let Err(e) = result {
        eprintln!("cannot open {target}: {e}");
    }
}

pub fn sanitize_folder_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').to_string();
    if trimmed.is_empty() {
        "instance".to_string()
    } else {
        trimmed
    }
}

pub fn classpath_separator() -> &'static str {
    if cfg!(windows) {
        ";"
    } else {
        ":"
    }
}

pub fn data_dir() -> PathBuf {
    if let Ok(custom) = std::env::var("SLINTLAUNCHER_HOME") {
        return PathBuf::from(custom);
    }
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("SlintLauncher")
}
