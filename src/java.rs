use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, Result};

pub fn resolve_java(configured: &str) -> Result<PathBuf> {
    if !configured.trim().is_empty() {
        let path = PathBuf::from(configured.trim());
        if path.exists() {
            return Ok(path);
        }
    }
    if let Ok(home) = std::env::var("JAVA_HOME") {
        let candidate = PathBuf::from(home).join("bin").join(java_bin());
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    if let Some(found) = which(java_bin()) {
        return Ok(found);
    }
    for extra in extra_locations() {
        if extra.exists() {
            return Ok(extra);
        }
    }
    Err(Error::JavaMissing)
}

pub fn describe(path: &Path) -> String {
    let output = Command::new(path).arg("-version").output().ok();
    let Some(output) = output else {
        return path.display().to_string();
    };
    let text = if output.stderr.is_empty() {
        String::from_utf8_lossy(&output.stdout).into_owned()
    } else {
        String::from_utf8_lossy(&output.stderr).into_owned()
    };
    text.lines()
        .next()
        .unwrap_or("Java")
        .trim()
        .to_string()
}

pub fn major_version(path: &Path) -> Option<u32> {
    let output = Command::new(path).arg("-version").output().ok()?;
    let text = String::from_utf8_lossy(&output.stderr);
    parse_major(&text).or_else(|| parse_major(&String::from_utf8_lossy(&output.stdout)))
}

fn parse_major(text: &str) -> Option<u32> {
    let start = text.find('"')? + 1;
    let end = text[start..].find('"')? + start;
    let ver = &text[start..end];
    if let Some(rest) = ver.strip_prefix("1.") {
        rest.split('.').next()?.parse().ok()
    } else {
        ver.split('.').next()?.parse().ok()
    }
}

fn java_bin() -> &'static str {
    if cfg!(windows) {
        "java.exe"
    } else {
        "java"
    }
}

fn which(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn extra_locations() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(windows) {
        out.push(PathBuf::from(
            r"C:\Program Files\Eclipse Adoptium\jdk-21\bin\java.exe",
        ));
        out.push(PathBuf::from(r"C:\Program Files\Java\jdk-21\bin\java.exe"));
    } else if cfg!(target_os = "macos") {
        out.push(PathBuf::from(
            "/Library/Java/JavaVirtualMachines/temurin-21.jdk/Contents/Home/bin/java",
        ));
    } else {
        for path in [
            "/usr/lib/jvm/java-21-openjdk/bin/java",
            "/usr/lib/jvm/java-21-openjdk-amd64/bin/java",
            "/usr/lib/jvm/java-17-openjdk/bin/java",
            "/usr/lib/jvm/java-17-openjdk-amd64/bin/java",
            "/usr/bin/java",
        ] {
            out.push(PathBuf::from(path));
        }
    }
    out
}
