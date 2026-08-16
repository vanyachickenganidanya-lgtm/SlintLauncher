//! Building the java command line and running the game.

use super::install::{resolve_libraries, Paths};
use super::manifest::*;
use crate::config::{Account, Settings};
use crate::ely;
use crate::util::classpath_separator;
use anyhow::{anyhow, bail, Result};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct LaunchSpec<'a> {
    pub paths: &'a Paths,
    pub detail: &'a VersionDetail,
    pub instance_dir: PathBuf,
    pub account: &'a Account,
    pub settings: &'a Settings,
    pub authlib_injector: Option<PathBuf>,
}

/// Find a usable `java` executable.
pub fn detect_java(configured: &str) -> Option<PathBuf> {
    if !configured.trim().is_empty() {
        let p = PathBuf::from(configured.trim());
        if p.exists() {
            return Some(p);
        }
    }

    if let Ok(home) = std::env::var("JAVA_HOME") {
        let candidate = PathBuf::from(home).join("bin").join(java_exe());
        if candidate.exists() {
            return Some(candidate);
        }
    }

    // PATH lookup
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(java_exe());
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }

    // Common install locations
    let roots: Vec<PathBuf> = if cfg!(windows) {
        vec![
            PathBuf::from(r"C:\Program Files\Java"),
            PathBuf::from(r"C:\Program Files\Eclipse Adoptium"),
            PathBuf::from(r"C:\Program Files\Microsoft\jdk"),
            PathBuf::from(r"C:\Program Files (x86)\Java"),
        ]
    } else if cfg!(target_os = "macos") {
        vec![PathBuf::from("/Library/Java/JavaVirtualMachines")]
    } else {
        vec![
            PathBuf::from("/usr/lib/jvm"),
            PathBuf::from("/usr/java"),
            PathBuf::from("/opt/java"),
        ]
    };

    let mut best: Option<PathBuf> = None;
    for root in roots {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let mut candidate = entry.path().join("bin").join(java_exe());
            if !candidate.exists() {
                candidate = entry.path().join("Contents/Home/bin").join(java_exe());
            }
            if candidate.exists() {
                best = Some(candidate);
            }
        }
    }
    best
}

pub fn java_exe() -> &'static str {
    if cfg!(windows) {
        "javaw.exe"
    } else {
        "java"
    }
}

pub fn java_version_string(java: &Path) -> String {
    match Command::new(java).arg("-version").output() {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stderr);
            text.lines().next().unwrap_or("").trim().to_string()
        }
        Err(e) => format!("java -version failed: {e}"),
    }
}

fn placeholder_map(spec: &LaunchSpec, classpath: &str, natives_dir: &Path) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let assets_root = spec.paths.assets();
    let assets_index = spec
        .detail
        .asset_index
        .as_ref()
        .map(|a| a.id.clone())
        .or_else(|| spec.detail.assets.clone())
        .unwrap_or_else(|| "legacy".into());

    map.insert("auth_player_name".into(), spec.account.username.clone());
    map.insert("version_name".into(), spec.detail.id.clone());
    map.insert("game_directory".into(), spec.instance_dir.display().to_string());
    map.insert("assets_root".into(), assets_root.display().to_string());
    map.insert("game_assets".into(), assets_root.join("virtual").join(&assets_index).display().to_string());
    map.insert("assets_index_name".into(), assets_index);
    map.insert("auth_uuid".into(), spec.account.uuid.replace('-', ""));
    map.insert(
        "auth_access_token".into(),
        if spec.account.access_token.is_empty() {
            "0".into()
        } else {
            spec.account.access_token.clone()
        },
    );
    map.insert("auth_session".into(), format!("token:{}:{}", spec.account.access_token, spec.account.uuid.replace('-', "")));
    map.insert(
        "user_type".into(),
        if spec.account.is_ely() { "mojang".into() } else { "legacy".to_string() },
    );
    map.insert("version_type".into(), spec.detail.kind.clone());
    map.insert("user_properties".into(), "{}".into());
    map.insert("classpath".into(), classpath.to_string());
    map.insert("natives_directory".into(), natives_dir.display().to_string());
    map.insert("launcher_name".into(), "SlintLauncher".into());
    map.insert("launcher_version".into(), env!("CARGO_PKG_VERSION").into());
    map.insert("classpath_separator".into(), classpath_separator().into());
    map.insert("library_directory".into(), spec.paths.libraries().display().to_string());
    map.insert("resolution_width".into(), spec.settings.window_width.to_string());
    map.insert("resolution_height".into(), spec.settings.window_height.to_string());
    map.insert("clientid".into(), String::new());
    map.insert("auth_xuid".into(), String::new());
    map
}

fn substitute(text: &str, map: &HashMap<String, String>) -> String {
    let mut out = text.to_string();
    for (key, value) in map {
        out = out.replace(&format!("${{{key}}}"), value);
    }
    out
}

fn collect_arguments(items: &[serde_json::Value], features: &HashMap<String, bool>) -> Vec<String> {
    let mut out = Vec::new();
    for item in items {
        match item {
            serde_json::Value::String(s) => out.push(s.clone()),
            serde_json::Value::Object(obj) => {
                let rules: Option<Vec<Rule>> = obj
                    .get("rules")
                    .and_then(|r| serde_json::from_value(r.clone()).ok());
                if !rules_allow(&rules, features) {
                    continue;
                }
                match obj.get("value") {
                    Some(serde_json::Value::String(s)) => out.push(s.clone()),
                    Some(serde_json::Value::Array(items)) => {
                        for v in items {
                            if let Some(s) = v.as_str() {
                                out.push(s.to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    out
}

pub struct BuiltCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

pub fn build_command(spec: &LaunchSpec) -> Result<BuiltCommand> {
    let java = detect_java(&spec.settings.java_path)
        .ok_or_else(|| anyhow!("Java не найдена. Укажите путь в настройках. / Java not found — set the path in settings."))?;

    let resolved = resolve_libraries(spec.paths, spec.detail);
    let mut classpath_entries: Vec<String> = resolved
        .classpath
        .iter()
        .filter(|p| p.exists())
        .map(|p| p.display().to_string())
        .collect();
    classpath_entries.push(spec.paths.version_jar(&spec.detail.id).display().to_string());
    let classpath = classpath_entries.join(classpath_separator());

    let natives_dir = spec.paths.natives(&spec.detail.id);
    let map = placeholder_map(spec, &classpath, &natives_dir);

    let mut features: HashMap<String, bool> = HashMap::new();
    features.insert("is_demo_user".into(), false);
    features.insert(
        "has_custom_resolution".into(),
        !spec.settings.fullscreen,
    );
    features.insert("has_quick_plays_support".into(), false);
    features.insert("is_quick_play_singleplayer".into(), false);
    features.insert("is_quick_play_multiplayer".into(), false);
    features.insert("is_quick_play_realms".into(), false);

    let mut args: Vec<String> = Vec::new();

    // --- authlib-injector must come before everything else Java-side
    if spec.account.is_ely() {
        if let Some(jar) = &spec.authlib_injector {
            args.push(format!("-javaagent:{}={}", jar.display(), ely::API_ROOT));
            args.push("-Dauthlibinjector.side=client".into());
            args.push("-Dauthlibinjector.noShowServerName".into());
        } else {
            bail!("authlib-injector is required for Ely.by accounts but is missing");
        }
    }

    // --- memory + user JVM args
    args.push(format!("-Xmx{}M", spec.settings.max_memory.max(512)));
    args.push(format!("-Xms{}M", (spec.settings.max_memory / 4).clamp(256, 2048)));
    for extra in spec.settings.jvm_args.split_whitespace() {
        args.push(extra.to_string());
    }

    if cfg!(target_os = "macos") {
        args.push("-XstartOnFirstThread".into());
    }

    // --- version supplied JVM args (or the legacy defaults)
    match &spec.detail.arguments {
        Some(arguments) if !arguments.jvm.is_empty() => {
            for arg in collect_arguments(&arguments.jvm, &features) {
                args.push(substitute(&arg, &map));
            }
        }
        _ => {
            args.push(format!("-Djava.library.path={}", natives_dir.display()));
            args.push("-cp".into());
            args.push(classpath.clone());
        }
    }

    if !args.iter().any(|a| a == "-cp" || a == "-classpath") {
        args.push("-cp".into());
        args.push(classpath.clone());
    }

    args.push(spec.detail.main_class.clone());

    // --- game arguments
    let game_args: Vec<String> = match (&spec.detail.arguments, &spec.detail.minecraft_arguments) {
        (Some(arguments), _) if !arguments.game.is_empty() => {
            collect_arguments(&arguments.game, &features)
        }
        (_, Some(legacy)) => legacy.split_whitespace().map(|s| s.to_string()).collect(),
        _ => Vec::new(),
    };
    for arg in game_args {
        args.push(substitute(&arg, &map));
    }

    // --- window size / fullscreen
    if spec.settings.fullscreen {
        if !args.iter().any(|a| a == "--fullscreen") {
            args.push("--fullscreen".into());
        }
    } else if !args.iter().any(|a| a == "--width") {
        args.push("--width".into());
        args.push(spec.settings.window_width.to_string());
        args.push("--height".into());
        args.push(spec.settings.window_height.to_string());
    }

    std::fs::create_dir_all(&spec.instance_dir)?;

    Ok(BuiltCommand {
        program: java,
        args,
        cwd: spec.instance_dir.clone(),
    })
}

/// Spawn the game and stream its output through `on_line`.
pub fn spawn<F>(cmd: BuiltCommand, on_line: F) -> Result<std::process::Child>
where
    F: Fn(String, bool) + Send + Sync + 'static,
{
    let mut child = Command::new(&cmd.program)
        .args(&cmd.args)
        .current_dir(&cmd.cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("cannot start Java ({}): {e}", cmd.program.display()))?;

    let callback = std::sync::Arc::new(on_line);

    if let Some(stdout) = child.stdout.take() {
        let cb = callback.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                cb(line, false);
            }
        });
    }

    if let Some(stderr) = child.stderr.take() {
        let cb = callback.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                cb(line, true);
            }
        });
    }

    Ok(child)
}
