//! SlintLauncher — нативный лаунчер Minecraft на Rust + Slint.
//!
//! This program is free software: you can redistribute it and/or modify
//! it under the terms of the GNU Affero General Public License as published
//! by the Free Software Foundation, either version 3 of the License, or
//! (at your option) any later version.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use slintlauncher::auth;
use slintlauncher::config::{self, Account, Config, Instance};
use slintlauncher::error::{self, Result};
use slintlauncher::fabric;
use slintlauncher::install::{self, Progress};
use slintlauncher::java;
use slintlauncher::launch;
use slintlauncher::mojang::{self, VersionInfo, VersionManifest};
use slintlauncher::news;
use slintlauncher::paths;

slint::include_modules!();

struct AppState {
    cfg: Mutex<Config>,
    manifest: Mutex<Option<VersionManifest>>,
    cancel_msa: Mutex<bool>,
}

fn runtime() -> &'static tokio::runtime::Runtime {
    use std::sync::OnceLock;
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("slintlauncher")
            .build()
            .expect("tokio")
    })
}

fn main() -> std::result::Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    let state = Arc::new(AppState {
        cfg: Mutex::new(Config::load()),
        manifest: Mutex::new(None),
        cancel_msa: Mutex::new(false),
    });

    {
        let cfg = state.cfg.lock().expect("cfg");
        apply_config_to_ui(&ui, &cfg);
        refresh_accounts(&ui, &cfg);
        refresh_instances(&ui, &cfg);
        refresh_hero(&ui, &cfg, None);
    }
    ui.set_data_dir(SharedString::from(paths::data_dir().display().to_string()));
    ui.set_java_label(SharedString::from(java_label(&state)));

    wire_callbacks(&ui, state.clone());
    bootstrap(ui.as_weak(), state);
    ui.run()
}

fn wire_callbacks(ui: &AppWindow, state: Arc<AppState>) {
    let weak = ui.as_weak();

    {
        let weak = weak.clone();
        ui.on_select_page(move |page| {
            if let Some(ui) = weak.upgrade() {
                ui.set_page(page);
            }
        });
    }

    {
        let weak = weak.clone();
        let state = state.clone();
        ui.on_apply_filter(move || {
            if let Some(ui) = weak.upgrade() {
                paint_versions(&ui, &state);
            }
        });
    }

    {
        let weak = weak.clone();
        let state = state.clone();
        ui.on_refresh(move || {
            fetch_catalog(weak.clone(), state.clone());
        });
    }

    {
        let weak = weak.clone();
        let state = state.clone();
        ui.on_select_version(move |id| {
            let id = id.to_string();
            if let Ok(mut cfg) = state.cfg.lock() {
                if let Some(inst) = cfg.active_instance_mut() {
                    inst.version = id;
                    inst.loader = "vanilla".into();
                    inst.fabric_loader.clear();
                }
                let _ = cfg.save();
                if let Some(ui) = weak.upgrade() {
                    refresh_instances(&ui, &cfg);
                    let manifest = manifest_clone(&state);
                    refresh_hero(&ui, &cfg, manifest.as_ref());
                    drop(cfg);
                    paint_versions(&ui, &state);
                    ui.set_page(0);
                }
            }
        });
    }

    {
        let weak = weak.clone();
        let state = state.clone();
        ui.on_select_instance(move |id| {
            if let Ok(mut cfg) = state.cfg.lock() {
                cfg.active_instance = id.to_string();
                let _ = cfg.save();
                if let Some(ui) = weak.upgrade() {
                    refresh_instances(&ui, &cfg);
                    let manifest = manifest_clone(&state);
                    refresh_hero(&ui, &cfg, manifest.as_ref());
                }
            }
        });
    }

    {
        let weak = weak.clone();
        let state = state.clone();
        ui.on_select_account(move |id| {
            if let Ok(mut cfg) = state.cfg.lock() {
                cfg.active_account = id.to_string();
                let _ = cfg.save();
                if let Some(ui) = weak.upgrade() {
                    refresh_accounts(&ui, &cfg);
                    let manifest = manifest_clone(&state);
                    refresh_hero(&ui, &cfg, manifest.as_ref());
                }
            }
        });
    }

    {
        let weak = weak.clone();
        let state = state.clone();
        ui.on_save_settings(move || {
            if let Some(ui) = weak.upgrade() {
                if let Ok(mut cfg) = state.cfg.lock() {
                    pull_settings(&ui, &mut cfg);
                    match cfg.save() {
                        Ok(()) => {
                            ui.set_status("Настройки сохранены".into());
                            ui.set_error_text("".into());
                        }
                        Err(e) => ui.set_error_text(e.to_string().into()),
                    }
                    let manifest = manifest_clone(&state);
                    refresh_hero(&ui, &cfg, manifest.as_ref());
                    drop(cfg);
                    ui.set_java_label(java_label(&state).into());
                }
            }
        });
    }

    {
        let weak = weak.clone();
        let state = state.clone();
        ui.on_create_instance(move || {
            if let Some(ui) = weak.upgrade() {
                let name = ui.get_new_name().to_string();
                let version = ui.get_new_version().to_string();
                let loader = ui.get_new_loader();
                if let Ok(mut cfg) = state.cfg.lock() {
                    let mut inst = Instance::vanilla(
                        if name.trim().is_empty() { "Инстанс" } else { name.trim() },
                        if version.trim().is_empty() {
                            "latest-release"
                        } else {
                            version.trim()
                        },
                    );
                    if loader == 1 {
                        inst.loader = "fabric".into();
                    }
                    cfg.active_instance = inst.id.clone();
                    cfg.instances.push(inst);
                    let _ = cfg.save();
                    ui.set_show_create(false);
                    refresh_instances(&ui, &cfg);
                    let manifest = manifest_clone(&state);
                    refresh_hero(&ui, &cfg, manifest.as_ref());
                    ui.set_status("Инстанс создан".into());
                }
            }
        });
    }

    {
        let weak = weak.clone();
        let state = state.clone();
        ui.on_delete_instance(move || {
            if let Ok(mut cfg) = state.cfg.lock() {
                if cfg.instances.len() <= 1 {
                    if let Some(ui) = weak.upgrade() {
                        ui.set_error_text("Нельзя удалить последний инстанс".into());
                    }
                    return;
                }
                let id = cfg.active_instance.clone();
                cfg.instances.retain(|i| i.id != id);
                cfg.active_instance = cfg.instances[0].id.clone();
                let _ = cfg.save();
                if let Some(ui) = weak.upgrade() {
                    refresh_instances(&ui, &cfg);
                    let manifest = manifest_clone(&state);
                    refresh_hero(&ui, &cfg, manifest.as_ref());
                    ui.set_error_text("".into());
                }
            }
        });
    }

    {
        let weak = weak.clone();
        let state = state.clone();
        ui.on_add_offline(move || {
            if let Some(ui) = weak.upgrade() {
                let name = ui.get_offline_name().to_string();
                if let Ok(mut cfg) = state.cfg.lock() {
                    let acc = Account::offline(&name);
                    cfg.active_account = acc.id.clone();
                    cfg.accounts.push(acc);
                    let _ = cfg.save();
                    refresh_accounts(&ui, &cfg);
                    refresh_hero(&ui, &cfg, None);
                }
            }
        });
    }

    {
        let weak = weak.clone();
        let state = state.clone();
        ui.on_delete_account(move || {
            if let Ok(mut cfg) = state.cfg.lock() {
                if cfg.accounts.len() <= 1 {
                    return;
                }
                let id = cfg.active_account.clone();
                cfg.accounts.retain(|a| a.id != id);
                cfg.active_account = cfg.accounts[0].id.clone();
                let _ = cfg.save();
                if let Some(ui) = weak.upgrade() {
                    refresh_accounts(&ui, &cfg);
                    refresh_hero(&ui, &cfg, None);
                }
            }
        });
    }

    {
        let weak = weak.clone();
        let state = state.clone();
        ui.on_start_msa(move || {
            start_microsoft(weak.clone(), state.clone());
        });
    }

    {
        let state = state.clone();
        ui.on_cancel_msa(move || {
            if let Ok(mut flag) = state.cancel_msa.lock() {
                *flag = true;
            }
        });
    }

    ui.on_open_msa_link({
        let weak = weak.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                let url = ui.get_msa_url().to_string();
                let _ = open::that(url);
            }
        }
    });

    ui.on_open_folder({
        let state = state.clone();
        move |which| {
            let which = which.to_string();
            let cfg = state.cfg.lock().ok();
            let game = cfg
                .as_ref()
                .and_then(|c| c.active_instance())
                .map(|i| i.game_dir());
            let path = match which.as_str() {
                "mods" => game.map(|g| g.join("mods")),
                "saves" => game.map(|g| g.join("saves")),
                "game" => game,
                _ => Some(paths::data_dir()),
            };
            if let Some(path) = path {
                launch::open_path(&path);
            }
        }
    });

    ui.on_play({
        let weak = weak.clone();
        let state = state.clone();
        move || play_clicked(weak.clone(), state.clone())
    });
}

fn bootstrap(ui: slint::Weak<AppWindow>, state: Arc<AppState>) {
    fetch_catalog(ui.clone(), state.clone());
    runtime().spawn(async move {
        let Ok(client) = mojang::http_client() else {
            return;
        };
        if let Ok(items) = news::fetch(&client).await {
            let rows: Vec<NewsRow> = items
                .into_iter()
                .map(|n| NewsRow {
                    title: n.title.into(),
                    body: n.body.into(),
                    version: n.version.into(),
                })
                .collect();
            invoke(ui, move |ui| {
                ui.set_news(ModelRc::new(VecModel::from(rows)));
            });
        }
    });
}

fn fetch_catalog(ui: slint::Weak<AppWindow>, state: Arc<AppState>) {
    invoke(ui.clone(), |ui| {
        ui.set_status("Обновляю каталог версий…".into());
    });
    runtime().spawn(async move {
        match load_manifest().await {
            Ok(manifest) => {
                if let Ok(mut slot) = state.manifest.lock() {
                    *slot = Some(manifest);
                }
                invoke(ui, move |ui| {
                    let manifest = manifest_clone(&state);
                    if let Ok(cfg) = state.cfg.lock() {
                        refresh_hero(ui, &cfg, manifest.as_ref());
                    }
                    paint_versions(ui, &state);
                    ui.set_status("Каталог готов".into());
                    ui.set_error_text("".into());
                });
            }
            Err(err) => {
                invoke(ui, move |ui| {
                    ui.set_status("Нет сети — работаем офлайн".into());
                    ui.set_error_text(err.to_string().into());
                });
            }
        }
    });
}

async fn load_manifest() -> Result<VersionManifest> {
    let client = mojang::http_client()?;
    mojang::fetch_manifest(&client).await
}

fn play_clicked(ui: slint::Weak<AppWindow>, state: Arc<AppState>) {
    if let Some(handle) = ui.upgrade() {
        if handle.get_busy() {
            return;
        }
        if let Ok(mut cfg) = state.cfg.lock() {
            pull_settings(&handle, &mut cfg);
            let _ = cfg.save();
        }
        handle.set_busy(true);
        handle.set_error_text("".into());
        handle.set_play_label("Готовлю…".into());
        handle.set_progress(0.0);
    }

    runtime().spawn(async move {
        let outcome = run_play(ui.clone(), state.clone()).await;
        invoke(ui, move |ui| {
            ui.set_busy(false);
            ui.set_play_label("Играть".into());
            ui.set_progress(0.0);
            ui.set_progress_text("".into());
            match outcome {
                Ok(msg) => {
                    ui.set_status(msg.into());
                    if ui.get_close_on_launch() {
                        let _ = ui.hide();
                    }
                }
                Err(err) => {
                    ui.set_error_text(err.to_string().into());
                    ui.set_status("Запуск не удался".into());
                }
            }
        });
    });
}

async fn run_play(ui: slint::Weak<AppWindow>, state: Arc<AppState>) -> Result<String> {
    let client = mojang::http_client()?;
    let (instance, account, ram) = {
        let cfg = state.cfg.lock().expect("cfg");
        let inst = cfg
            .active_instance()
            .cloned()
            .ok_or_else(|| error::Error::msg("нет инстанса"))?;
        let acc = cfg
            .active_account()
            .cloned()
            .ok_or(error::Error::NoAccount)?;
        (inst, acc, cfg.ram_mb)
    };

    let mut account = account;
    if account.kind == config::AccountKind::Microsoft {
        match auth::refresh_account(&client, &account).await {
            Ok(fresh) => {
                account = fresh;
                if let Ok(mut cfg) = state.cfg.lock() {
                    if let Some(slot) = cfg
                        .accounts
                        .iter_mut()
                        .find(|a| a.id == account.id)
                    {
                        *slot = account.clone();
                    }
                    let _ = cfg.save();
                }
            }
            Err(err) => {
                return Err(error::Error::Auth(format!(
                    "не удалось обновить Microsoft-сессию: {err}"
                )));
            }
        }
    }

    let manifest = match manifest_clone(&state) {
        Some(m) => m,
        None => mojang::fetch_manifest(&client).await?,
    };
    if let Ok(mut slot) = state.manifest.lock() {
        *slot = Some(manifest.clone());
    }

    let mut instance = instance;
    if instance.version == "latest-release" {
        instance.version = manifest.latest.release.clone();
    } else if instance.version == "latest-snapshot" {
        instance.version = manifest.latest.snapshot.clone();
    }

    report(&ui, "Проверяю файлы…", 0.02, "манифест версии");

    let meta = if instance.loader == "fabric" {
        let loader = if instance.fabric_loader.is_empty() {
            let latest = fabric::latest_loader(&client, &instance.version).await?;
            if let Ok(mut cfg) = state.cfg.lock() {
                if let Some(slot) = cfg.instances.iter_mut().find(|i| i.id == instance.id) {
                    slot.fabric_loader = latest.clone();
                    slot.version = instance.version.clone();
                    let _ = cfg.save();
                }
            }
            latest
        } else {
            instance.fabric_loader.clone()
        };
        report(&ui, "Ставлю Fabric…", 0.05, &loader);
        fabric::ensure_fabric(&client, &manifest, &instance.version, &loader).await?
    } else {
        mojang::resolve_version(&client, &manifest, &instance.version).await?
    };

    let ui_progress = ui.clone();
    let on_progress: install::ProgressFn = Arc::new(move |p: Progress| {
        let fraction = if p.total == 0 {
            0.0
        } else {
            p.current as f32 / p.total as f32
        };
        report(
            &ui_progress,
            &format!("{} — {}", p.stage, p.detail),
            0.08 + 0.82 * fraction,
            &p.stage,
        );
    });

    install::install_version(&client, &meta, on_progress).await?;
    report(&ui, "Запускаю игру…", 0.95, "Java");

    let cfg = state.cfg.lock().expect("cfg").clone();
    let plan = launch::prepare(&cfg, &instance, &account, &meta)?;
    let pid = launch::spawn(&plan)?;

    if let Ok(mut cfg) = state.cfg.lock() {
        if let Some(slot) = cfg.instances.iter_mut().find(|i| i.id == instance.id) {
            slot.last_played = chrono::Utc::now().timestamp();
            slot.version = instance.version.clone();
        }
        let _ = cfg.save();
    }

    let _ = ram;
    Ok(format!("Minecraft запущен (pid {pid})"))
}

fn start_microsoft(ui: slint::Weak<AppWindow>, state: Arc<AppState>) {
    if let Ok(mut flag) = state.cancel_msa.lock() {
        *flag = false;
    }
    invoke(ui.clone(), |ui| {
        ui.set_msa_pending(true);
        ui.set_msa_code("……".into());
        ui.set_status("Запрашиваю код Microsoft…".into());
    });
    runtime().spawn(async move {
        let client = match mojang::http_client() {
            Ok(c) => c,
            Err(e) => {
                fail_msa(ui, &e.to_string());
                return;
            }
        };
        let device = match auth::start_device_code(&client).await {
            Ok(d) => d,
            Err(e) => {
                fail_msa(ui, &e.to_string());
                return;
            }
        };
        let code = device.user_code.clone();
        let url = device.verification_uri.clone();
        invoke(ui.clone(), move |ui| {
            ui.set_msa_code(code.into());
            ui.set_msa_url(url.into());
            ui.set_status("Ожидаю вход Microsoft…".into());
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(device.expires_in);
        loop {
            if state.cancel_msa.lock().map(|g| *g).unwrap_or(true) {
                invoke(ui, |ui| {
                    ui.set_msa_pending(false);
                    ui.set_status("Вход отменён".into());
                });
                return;
            }
            if std::time::Instant::now() > deadline {
                fail_msa(ui, "время на вход истекло");
                return;
            }
            match auth::poll_device_code(&client, &device).await {
                Ok(Some(account)) => {
                    if let Ok(mut cfg) = state.cfg.lock() {
                        cfg.active_account = account.id.clone();
                        if let Some(existing) = cfg.accounts.iter_mut().find(|a| a.uuid == account.uuid)
                        {
                            *existing = account;
                        } else {
                            cfg.accounts.push(account);
                        }
                        let _ = cfg.save();
                    }
                    invoke(ui, move |ui| {
                        ui.set_msa_pending(false);
                        if let Ok(cfg) = state.cfg.lock() {
                            refresh_accounts(ui, &cfg);
                            refresh_hero(ui, &cfg, None);
                        }
                        ui.set_status("Microsoft-аккаунт добавлен".into());
                        ui.set_error_text("".into());
                    });
                    return;
                }
                Ok(None) => {
                    tokio::time::sleep(Duration::from_secs(device.interval)).await;
                }
                Err(e) => {
                    fail_msa(ui, &e.to_string());
                    return;
                }
            }
        }
    });
}

fn fail_msa(ui: slint::Weak<AppWindow>, message: &str) {
    let message = message.to_string();
    invoke(ui, move |ui| {
        ui.set_msa_pending(false);
        ui.set_error_text(message.clone().into());
        ui.set_status("Вход Microsoft не удался".into());
    });
}

fn report(ui: &slint::Weak<AppWindow>, status: &str, progress: f32, detail: &str) {
    let status = status.to_string();
    let detail = detail.to_string();
    invoke(ui.clone(), move |ui| {
        ui.set_status(status.into());
        ui.set_progress(progress);
        ui.set_progress_text(detail.into());
        ui.set_play_label("Загрузка…".into());
    });
}

fn invoke(ui: slint::Weak<AppWindow>, f: impl FnOnce(&AppWindow) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = ui.upgrade() {
            f(&ui);
        }
    });
}

fn apply_config_to_ui(ui: &AppWindow, cfg: &Config) {
    ui.set_ram_mb(cfg.ram_mb as i32);
    ui.set_java_path(cfg.java_path.clone().into());
    ui.set_jvm_args(cfg.jvm_args.clone().into());
    ui.set_width_text(cfg.width.to_string().into());
    ui.set_height_text(cfg.height.to_string().into());
    ui.set_fullscreen(cfg.fullscreen);
    ui.set_close_on_launch(cfg.close_on_launch);
    ui.set_show_snapshots(cfg.show_snapshots);
    ui.set_show_old(cfg.show_old);
}

fn pull_settings(ui: &AppWindow, cfg: &mut Config) {
    cfg.ram_mb = ui.get_ram_mb().clamp(1024, 32768) as u32;
    cfg.java_path = ui.get_java_path().to_string();
    cfg.jvm_args = ui.get_jvm_args().to_string();
    cfg.width = parse_num(&ui.get_width_text(), 1280).clamp(640, 7680);
    cfg.height = parse_num(&ui.get_height_text(), 720).clamp(480, 4320);
    cfg.fullscreen = ui.get_fullscreen();
    cfg.close_on_launch = ui.get_close_on_launch();
    cfg.show_snapshots = ui.get_show_snapshots();
    cfg.show_old = ui.get_show_old();
}

fn parse_num(text: &str, fallback: u32) -> u32 {
    text.chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .unwrap_or(fallback)
}

fn refresh_accounts(ui: &AppWindow, cfg: &Config) {
    let rows: Vec<AccountRow> = cfg
        .accounts
        .iter()
        .map(|a| AccountRow {
            id: a.id.clone().into(),
            name: a.name.clone().into(),
            kind: a.kind_label().into(),
            selected: a.id == cfg.active_account,
        })
        .collect();
    ui.set_accounts(ModelRc::new(VecModel::from(rows)));
}

fn refresh_instances(ui: &AppWindow, cfg: &Config) {
    let rows: Vec<InstanceRow> = cfg
        .instances
        .iter()
        .map(|i| InstanceRow {
            id: i.id.clone().into(),
            name: i.name.clone().into(),
            version: display_version(i, None).into(),
            loader: i.loader_label().into(),
            last_played: format_played(i.last_played).into(),
            selected: i.id == cfg.active_instance,
        })
        .collect();
    ui.set_instances(ModelRc::new(VecModel::from(rows)));
}

fn refresh_hero(ui: &AppWindow, cfg: &Config, manifest: Option<&VersionManifest>) {
    let inst = cfg.active_instance();
    let acc = cfg.active_account();
    ui.set_hero_instance(
        inst.map(|i| i.name.as_str())
            .unwrap_or("Инстанс")
            .into(),
    );
    ui.set_hero_version(inst.map(|i| display_version(i, manifest)).unwrap_or_else(|| "—".into()).into());
    ui.set_hero_loader(inst.map(|i| i.loader_label()).unwrap_or_else(|| "Vanilla".into()).into());
    ui.set_hero_account(acc.map(|a| a.name.as_str()).unwrap_or("Player").into());
    ui.set_ram_mb(cfg.ram_mb as i32);
}

fn display_version(inst: &Instance, manifest: Option<&VersionManifest>) -> String {
    match inst.version.as_str() {
        "latest-release" => manifest
            .map(|m| m.latest.release.clone())
            .unwrap_or_else(|| "latest-release".into()),
        "latest-snapshot" => manifest
            .map(|m| m.latest.snapshot.clone())
            .unwrap_or_else(|| "latest-snapshot".into()),
        other => other.to_string(),
    }
}

fn format_played(ts: i64) -> String {
    if ts <= 0 {
        return "ещё не запускался".into();
    }
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|dt| dt.format("%d.%m.%Y %H:%M").to_string())
        .unwrap_or_else(|| "—".into())
}

fn manifest_clone(state: &AppState) -> Option<VersionManifest> {
    state.manifest.lock().ok().and_then(|guard| guard.clone())
}

fn paint_versions(ui: &AppWindow, state: &AppState) {
    let Some(manifest) = state.manifest.lock().ok().and_then(|g| g.clone()) else {
        ui.set_versions(ModelRc::new(VecModel::from(Vec::<VersionRow>::new())));
        return;
    };
    let query = ui.get_version_query().to_string().to_ascii_lowercase();
    let filter = ui.get_version_filter();
    let selected = state
        .cfg
        .lock()
        .ok()
        .and_then(|c| c.active_instance().map(|i| i.version.clone()))
        .unwrap_or_default();

    let rows: Vec<VersionRow> = manifest
        .versions
        .iter()
        .filter(|v| version_visible(v, filter, &query))
        .take(400)
        .map(|v| {
            let installed = paths::version_jar(&v.id).exists();
            VersionRow {
                id: v.id.clone().into(),
                kind: v.kind_label().into(),
                date: v.date_short().into(),
                installed,
                selected: v.id == selected,
            }
        })
        .collect();
    ui.set_versions(ModelRc::new(VecModel::from(rows)));
}

fn version_visible(info: &VersionInfo, filter: i32, query: &str) -> bool {
    let kind_ok = match filter {
        0 => info.kind == "release",
        1 => info.kind == "snapshot",
        2 => info.kind == "old_beta" || info.kind == "old_alpha",
        _ => true,
    };
    if !kind_ok {
        return false;
    }
    if query.is_empty() {
        return true;
    }
    info.id.to_ascii_lowercase().contains(query)
}

fn java_label(state: &AppState) -> String {
    let configured = state
        .cfg
        .lock()
        .map(|c| c.java_path.clone())
        .unwrap_or_default();
    match java::resolve_java(&configured) {
        Ok(path) => java::describe(&path),
        Err(_) => "Java не найдена".into(),
    }
}
