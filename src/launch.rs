use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::config::{Account, Config, Instance};
use crate::error::{Error, Result};
use crate::install;
use crate::java;
use crate::mojang::{self, argument_strings, VersionMeta};
use crate::paths;
use crate::rules::Features;

pub struct LaunchPlan {
    pub java: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

pub fn prepare(
    cfg: &Config,
    instance: &Instance,
    account: &Account,
    meta: &VersionMeta,
) -> Result<LaunchPlan> {
    let java_path = java::resolve_java(&cfg.java_path)?;
    if let Some(required) = meta.java_version.as_ref().map(|j| j.major_version) {
        if required > 0 {
            if let Some(have) = java::major_version(&java_path) {
                if have < required {
                    return Err(Error::msg(format!(
                        "нужна Java {required}+, найдена {have}"
                    )));
                }
            }
        }
    }

    let natives = install::extract_natives(meta)?;
    let classpath = install::classpath(meta)?;
    let game_dir = instance.game_dir();
    crate::paths::ensure_dir(&game_dir)?;
    for extra in ["saves", "mods", "resourcepacks", "shaderpacks", "screenshots", "logs"] {
        let _ = crate::paths::ensure_dir(&game_dir.join(extra));
    }

    let features = Features {
        demo: false,
        custom_resolution: true,
    };
    let ram = instance.ram_mb(cfg.ram_mb);
    let mut vars = HashMap::new();
    vars.insert("auth_player_name".into(), account.name.clone());
    vars.insert("version_name".into(), meta.id.clone());
    vars.insert("game_directory".into(), game_dir.to_string_lossy().into());
    vars.insert(
        "assets_root".into(),
        paths::assets_dir().to_string_lossy().into(),
    );
    vars.insert(
        "game_assets".into(),
        paths::assets_dir().to_string_lossy().into(),
    );
    vars.insert(
        "assets_index_name".into(),
        meta.asset_index
            .as_ref()
            .map(|a| a.id.clone())
            .or_else(|| meta.assets.clone())
            .unwrap_or_else(|| "legacy".into()),
    );
    vars.insert("auth_uuid".into(), account.uuid.replace('-', ""));
    vars.insert(
        "auth_access_token".into(),
        if account.access_token.is_empty() {
            "0".into()
        } else {
            account.access_token.clone()
        },
    );
    vars.insert("auth_session".into(), account.access_token.clone());
    vars.insert("user_type".into(), account.user_type().into());
    vars.insert("version_type".into(), meta.kind.clone());
    vars.insert(
        "natives_directory".into(),
        natives.to_string_lossy().into(),
    );
    vars.insert("launcher_name".into(), "SlintLauncher".into());
    vars.insert(
        "launcher_version".into(),
        env!("CARGO_PKG_VERSION").into(),
    );
    vars.insert("classpath".into(), classpath);
    vars.insert("user_properties".into(), "{}".into());
    vars.insert("clientid".into(), "slintlauncher".into());
    vars.insert("auth_xuid".into(), account.xuid.clone());
    vars.insert("resolution_width".into(), cfg.width.to_string());
    vars.insert("resolution_height".into(), cfg.height.to_string());
    vars.insert("library_directory".into(), paths::libraries_dir().to_string_lossy().into());
    vars.insert("classpath_separator".into(), paths::classpath_sep().into());

    if let Some(logging) = meta.logging.as_ref().and_then(|l| l.client.as_ref()) {
        if let Some(name) = logging
            .file
            .url
            .as_ref()
            .and_then(|u| u.rsplit('/').next())
            .map(|s| s.to_string())
            .or_else(|| logging.file.path.clone())
        {
            let log_path = paths::assets_dir().join("log_configs").join(name);
            vars.insert("path".into(), log_path.to_string_lossy().into());
        }
    }

    let mut args = Vec::new();
    args.push(format!("-Xms{}M", (ram / 4).max(512)));
    args.push(format!("-Xmx{ram}M"));
    for extra in cfg.jvm_args.split_whitespace() {
        args.push(extra.to_string());
    }

    if let Some(arguments) = &meta.arguments {
        args.extend(
            argument_strings(&arguments.jvm, &features)
                .into_iter()
                .map(|s| substitute(&s, &vars)),
        );
    } else {
        args.push(format!(
            "-Djava.library.path={}",
            natives.to_string_lossy()
        ));
        args.push("-Dminecraft.launcher.brand=SlintLauncher".into());
        args.push(format!(
            "-Dminecraft.launcher.version={}",
            env!("CARGO_PKG_VERSION")
        ));
        args.push("-cp".into());
        args.push(vars["classpath"].clone());
    }

    if let Some(logging) = meta.logging.as_ref().and_then(|l| l.client.as_ref()) {
        if vars.contains_key("path") {
            args.push(substitute(&logging.argument, &vars));
        }
    }

    args.push(meta.main_class.clone());

    if let Some(arguments) = &meta.arguments {
        args.extend(
            argument_strings(&arguments.game, &features)
                .into_iter()
                .map(|s| substitute(&s, &vars)),
        );
    } else if let Some(legacy) = &meta.minecraft_arguments {
        args.extend(legacy.split_whitespace().map(|s| substitute(s, &vars)));
    } else {
        return Err(Error::msg("в манифесте версии нет аргументов запуска"));
    }

    if cfg.fullscreen {
        args.push("--fullscreen".into());
    }

    Ok(LaunchPlan {
        java: java_path,
        args,
        cwd: game_dir,
    })
}

pub fn spawn(plan: &LaunchPlan) -> Result<u32> {
    let mut cmd = Command::new(&plan.java);
    cmd.args(&plan.args)
        .current_dir(&plan.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let child = cmd
        .spawn()
        .map_err(|e| Error::msg(format!("не удалось запустить Java: {e}")))?;
    Ok(child.id())
}

fn substitute(template: &str, vars: &HashMap<String, String>) -> String {
    let mut out = template.to_string();
    for (key, value) in vars {
        out = out.replace(&format!("${{{key}}}"), value);
    }
    out
}

pub fn open_path(path: &Path) {
    let _ = crate::paths::ensure_dir(path);
    let _ = open::that(path);
}
