#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod config;
mod ely;
mod minecraft;
mod skin;
mod util;

use config::{Account, Instance, Settings, Store};
use minecraft::install::{self, Paths, Reporter};
use minecraft::launch;
use minecraft::manifest::ManifestEntry;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel, Weak};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

slint::include_modules!();

const MAX_LOG_LINES: usize = 4000;

/// Everything the worker threads need to touch, behind one lock.
struct AppState {
    store: Store,
    paths: Paths,
    versions: Vec<ManifestEntry>,
    running: HashMap<String, u32>,
    heads: HashMap<String, slint::Image>,
    busy: bool,
}

type Shared = Arc<Mutex<AppState>>;

fn main() -> Result<(), slint::PlatformError> {
    let root = util::data_dir();
    let _ = std::fs::create_dir_all(&root);

    let store = Store::load();
    let state: Shared = Arc::new(Mutex::new(AppState {
        store,
        paths: Paths::new(root.clone()),
        versions: Vec::new(),
        running: HashMap::new(),
        heads: HashMap::new(),
        busy: false,
    }));

    let ui = AppWindow::new()?;
    ui.set_app_version(env!("CARGO_PKG_VERSION").into());
    ui.set_data_dir(root.display().to_string().into());

    // apply persisted settings to the UI
    {
        let guard = state.lock().unwrap();
        let s = &guard.store.settings;
        ui.set_java_path(s.java_path.clone().into());
        ui.set_max_memory(s.max_memory as i32);
        ui.set_jvm_args(s.jvm_args.clone().into());
        ui.set_win_width(s.window_width as i32);
        ui.set_win_height(s.window_height as i32);
        ui.set_fullscreen(s.fullscreen);
        ui.set_hide_launcher(s.hide_launcher);
        ui.set_auto_java(s.auto_java);
        ui.global::<Tr>().set_ru(s.russian);
        ui.global::<Theme>().set_light(s.light_theme);
    }

    ui.set_log_lines(ModelRc::new(VecModel::<LogLine>::default()));
    ui.set_instances(ModelRc::new(VecModel::<InstanceItem>::default()));
    ui.set_accounts(ModelRc::new(VecModel::<AccountItem>::default()));
    ui.set_versions(ModelRc::new(VecModel::<VersionItem>::default()));

    refresh_instances(&ui, &state);
    refresh_accounts(&ui, &state);

    log_line(&ui, "info", &format!("SlintLauncher v{}", env!("CARGO_PKG_VERSION")));
    log_line(&ui, "info", &format!("Данные / Data: {}", root.display()));

    // Detect Java in the background so the settings page has something to show.
    {
        let weak = ui.as_weak();
        let state = state.clone();
        std::thread::spawn(move || {
            let configured = state.lock().unwrap().store.settings.java_path.clone();
            let found = launch::detect_java(&configured);
            let text = match &found {
                Some(path) => format!("{} — {}", path.display(), launch::java_version_string(path)),
                None => "Java не найдена / Java not found".to_string(),
            };
            let _ = weak.upgrade_in_event_loop(move |ui| {
                ui.set_detected_java(text.clone().into());
                log_line(&ui, if text.contains("not found") { "warn" } else { "ok" }, &format!("Java: {text}"));
            });
        });
    }

    // Load skins for stored Ely.by accounts.
    load_heads(&ui, &state);

    install_callbacks(&ui, &state);

    ui.run()
}

// ------------------------------------------------------------------ helpers

fn log_line(ui: &AppWindow, level: &str, text: &str) {
    let model = ui.get_log_lines();
    let Some(vec_model) = model.as_any().downcast_ref::<VecModel<LogLine>>() else {
        return;
    };
    let stamp = timestamp();
    vec_model.push(LogLine {
        level: level.into(),
        text: format!("[{stamp}] {text}").into(),
    });
    while vec_model.row_count() > MAX_LOG_LINES {
        vec_model.remove(0);
    }
}

fn log_from_thread(weak: &Weak<AppWindow>, level: &str, text: String) {
    let level = level.to_string();
    let _ = weak.upgrade_in_event_loop(move |ui| log_line(&ui, &level, &text));
}

fn timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let s = secs % 86400;
    format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

fn today() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / 86400)
        .unwrap_or(0);
    // civil-from-days (Howard Hinnant's algorithm)
    let z = days as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02}", y, m, d)
}

fn hue_for(text: &str) -> f32 {
    let mut h: u32 = 2166136261;
    for b in text.as_bytes() {
        h ^= *b as u32;
        h = h.wrapping_mul(16777619);
    }
    (h % 360) as f32
}

fn icon_text(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .take(2)
        .collect::<String>()
        .to_uppercase()
}

fn refresh_instances(ui: &AppWindow, state: &Shared) {
    let guard = state.lock().unwrap();
    let paths = &guard.paths;
    let items: Vec<InstanceItem> = guard
        .store
        .instances
        .iter()
        .map(|i| InstanceItem {
            id: i.id.clone().into(),
            name: i.name.clone().into(),
            version: i.version.clone().into(),
            kind: i.kind.clone().into(),
            installed: paths.version_jar(&i.version).exists(),
            running: guard.running.contains_key(&i.id),
            last_played: i.last_played.clone().into(),
            hue: hue_for(&i.name),
            icon_text: icon_text(&i.name).into(),
        })
        .collect();
    ui.set_instances(ModelRc::new(VecModel::from(items)));
}

fn refresh_accounts(ui: &AppWindow, state: &Shared) {
    let guard = state.lock().unwrap();
    let active_id = guard.store.active_account.clone();
    let empty = slint::Image::default();

    let items: Vec<AccountItem> = guard
        .store
        .accounts
        .iter()
        .map(|a| {
            let head = guard.heads.get(&a.id).cloned();
            AccountItem {
                id: a.id.clone().into(),
                username: a.username.clone().into(),
                uuid: a.uuid.clone().into(),
                kind: a.kind.clone().into(),
                active: a.id == active_id,
                has_head: head.is_some(),
                head: head.unwrap_or_else(|| empty.clone()),
            }
        })
        .collect();

    let active = guard.store.active();
    ui.set_active_account(active.map(|a| a.username.clone()).unwrap_or_default().into());
    ui.set_active_is_ely(active.map(|a| a.is_ely()).unwrap_or(false));
    match active.and_then(|a| guard.heads.get(&a.id).cloned()) {
        Some(head) => {
            ui.set_active_head(head);
            ui.set_active_has_head(true);
        }
        None => ui.set_active_has_head(false),
    }

    ui.set_accounts(ModelRc::new(VecModel::from(items)));
}

fn load_heads(ui: &AppWindow, state: &Shared) {
    let accounts: Vec<Account> = {
        let guard = state.lock().unwrap();
        guard
            .store
            .accounts
            .iter()
            .filter(|a| a.is_ely() && !guard.heads.contains_key(&a.id))
            .cloned()
            .collect()
    };
    if accounts.is_empty() {
        return;
    }

    let weak = ui.as_weak();
    let state = state.clone();
    std::thread::spawn(move || {
        for account in accounts {
            let Ok(png) = ely::fetch_skin(&account.username) else {
                continue;
            };
            let weak = weak.clone();
            let state = state.clone();
            let id = account.id.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Ok(head) = skin::head_from_skin(&png) {
                    state.lock().unwrap().heads.insert(id, head);
                    if let Some(ui) = weak.upgrade() {
                        refresh_accounts(&ui, &state);
                    }
                }
            });
        }
    });
}

fn set_busy(ui: &AppWindow, busy: bool, text: &str, progress: f32) {
    ui.set_busy(busy);
    ui.set_status_text(text.into());
    ui.set_progress(progress);
}

fn toast(weak: &Weak<AppWindow>, text: String) {
    let w = weak.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        ui.set_toast(text.clone().into());
        let w2 = w.clone();
        slint::Timer::single_shot(std::time::Duration::from_secs(3), move || {
            if let Some(ui) = w2.upgrade() {
                ui.set_toast(SharedString::new());
            }
        });
    });
}

fn save_store(state: &Shared) {
    if let Err(e) = state.lock().unwrap().store.save() {
        eprintln!("cannot save config: {e:#}");
    }
}

// -------------------------------------------------------------- callbacks

fn install_callbacks(ui: &AppWindow, state: &Shared) {
    // ---------------------------------------------------------- versions
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_refresh_versions(move |snapshots, old| {
            let weak = weak.clone();
            let state = state.clone();

            if let Some(ui) = weak.upgrade() {
                ui.set_versions_loading(true);
                ui.set_versions_error(SharedString::new());
            }

            std::thread::spawn(move || {
                let cached = state.lock().unwrap().versions.clone();
                let list = if cached.is_empty() {
                    match install::fetch_manifest() {
                        Ok(manifest) => {
                            state.lock().unwrap().versions = manifest.versions.clone();
                            manifest.versions
                        }
                        Err(e) => {
                            let msg = format!("{e:#}");
                            let _ = weak.upgrade_in_event_loop(move |ui| {
                                ui.set_versions_loading(false);
                                ui.set_versions_error(
                                    format!("Не удалось получить список версий / Cannot fetch versions: {msg}").into(),
                                );
                            });
                            return;
                        }
                    }
                } else {
                    cached
                };

                let filtered: Vec<VersionItem> = list
                    .iter()
                    .filter(|v| match v.kind.as_str() {
                        "release" => true,
                        "snapshot" => snapshots,
                        _ => old,
                    })
                    .map(|v| VersionItem {
                        id: v.id.clone().into(),
                        kind: v.kind.clone().into(),
                        released: v.release_time.chars().take(10).collect::<String>().into(),
                    })
                    .collect();

                let default_version = filtered.first().map(|v| v.id.clone());

                let _ = weak.upgrade_in_event_loop(move |ui| {
                    ui.set_versions(ModelRc::new(VecModel::from(filtered)));
                    ui.set_versions_loading(false);
                    if ui.get_new_instance_version().is_empty() {
                        if let Some(v) = default_version {
                            ui.set_new_instance_version(v);
                        }
                    }
                });
            });
        });
    }

    // ---------------------------------------------------- create instance
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_create_instance(move |name, version| {
            let name = name.to_string();
            let version = version.to_string();
            if name.trim().is_empty() || version.is_empty() {
                return;
            }

            let id = uuid::Uuid::new_v4().to_string();
            let folder = util::sanitize_folder_name(&name);
            let kind = state
                .lock()
                .unwrap()
                .versions
                .iter()
                .find(|v| v.id == version)
                .map(|v| v.kind.clone())
                .unwrap_or_else(|| "release".into());

            {
                let mut guard = state.lock().unwrap();
                guard.store.instances.push(Instance {
                    id: id.clone(),
                    name: name.clone(),
                    version: version.clone(),
                    kind,
                    last_played: String::new(),
                    folder,
                });
            }
            save_store(&state);

            if let Some(ui) = weak.upgrade() {
                refresh_instances(&ui, &state);
                ui.set_selected_instance(id.into());
                log_line(&ui, "ok", &format!("Создана сборка «{name}» ({version})"));
            }
        });
    }

    // ---------------------------------------------------- delete instance
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_delete_instance(move |id| {
            let id = id.to_string();
            let removed = {
                let mut guard = state.lock().unwrap();
                if guard.running.contains_key(&id) {
                    None
                } else if let Some(pos) = guard.store.instances.iter().position(|i| i.id == id) {
                    let inst = guard.store.instances.remove(pos);
                    let dir = guard.paths.instances().join(&inst.folder);
                    let _ = std::fs::remove_dir_all(dir);
                    Some(inst)
                } else {
                    None
                }
            };
            save_store(&state);
            if let Some(ui) = weak.upgrade() {
                refresh_instances(&ui, &state);
                match removed {
                    Some(inst) => log_line(&ui, "warn", &format!("Сборка «{}» удалена", inst.name)),
                    None => log_line(&ui, "error", "Нельзя удалить запущенную сборку"),
                }
            }
        });
    }

    // ------------------------------------------------------ open folder
    {
        let state = state.clone();
        ui.on_open_instance_folder(move |id| {
            let guard = state.lock().unwrap();
            if let Some(inst) = guard.store.instance(&id.to_string()) {
                let dir = guard.paths.instances().join(&inst.folder);
                let _ = std::fs::create_dir_all(&dir);
                util::open_path(&dir.display().to_string());
            }
        });
    }

    // ------------------------------------------------------------- play
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_play(move |id| {
            let id = id.to_string();
            let weak = weak.clone();
            let state = state.clone();

            {
                let guard = state.lock().unwrap();
                if guard.busy {
                    return;
                }
                if guard.store.active().is_none() {
                    drop(guard);
                    if let Some(ui) = weak.upgrade() {
                        log_line(&ui, "error", "Сначала войдите в аккаунт / Sign in first");
                        ui.invoke_open_login();
                    }
                    return;
                }
            }

            state.lock().unwrap().busy = true;
            if let Some(ui) = weak.upgrade() {
                set_busy(&ui, true, "Подготовка / Preparing...", 0.0);
            }

            std::thread::spawn(move || {
                let result = run_instance(&weak, &state, &id);
                state.lock().unwrap().busy = false;

                match result {
                    Ok(()) => {}
                    Err(e) => {
                        let msg = format!("{e:#}");
                        log_from_thread(&weak, "error", format!("Ошибка запуска / Launch failed: {msg}"));
                        toast(&weak, "Ошибка запуска / Launch failed".into());
                    }
                }

                let state2 = state.clone();
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    set_busy(&ui, false, "", 0.0);
                    refresh_instances(&ui, &state2);
                });
            });
        });
    }

    // -------------------------------------------------------- ely login
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_login_ely(move |login, password, totp| {
            let (login, password, totp) = (login.to_string(), password.to_string(), totp.to_string());
            let weak = weak.clone();
            let state = state.clone();

            if let Some(ui) = weak.upgrade() {
                ui.set_login_busy(true);
                ui.set_login_error(SharedString::new());
            }

            std::thread::spawn(move || {
                let client_token = uuid::Uuid::new_v4().to_string();
                match ely::login(&login, &password, &totp, &client_token) {
                    Ok(outcome) if outcome.needs_totp => {
                        let _ = weak.upgrade_in_event_loop(|ui| {
                            ui.set_login_busy(false);
                            ui.set_login_needs_totp(true);
                            ui.set_login_error(
                                "Введите код двухфакторной аутентификации / Enter your 2FA code".into(),
                            );
                        });
                    }
                    Ok(outcome) => {
                        let account = outcome.account;
                        let username = account.username.clone();
                        {
                            let mut guard = state.lock().unwrap();
                            guard.store.accounts.retain(|a| a.id != account.id);
                            guard.store.active_account = account.id.clone();
                            guard.store.accounts.push(account);
                        }
                        save_store(&state);

                        let state2 = state.clone();
                        let _ = weak.upgrade_in_event_loop(move |ui| {
                            ui.set_login_busy(false);
                            ui.set_login_needs_totp(false);
                            ui.set_login_error(SharedString::new());
                            ui.set_show_login(false);
                            refresh_accounts(&ui, &state2);
                            load_heads(&ui, &state2);
                            log_line(&ui, "ok", &format!("Вход выполнен: {username} (Ely.by)"));
                        });
                        toast(&weak, format!("Привет, {username}!"));
                    }
                    Err(e) => {
                        let msg = format!("{e:#}");
                        let _ = weak.upgrade_in_event_loop(move |ui| {
                            ui.set_login_busy(false);
                            ui.set_login_error(msg.clone().into());
                            log_line(&ui, "error", &format!("Ошибка входа: {msg}"));
                        });
                    }
                }
            });
        });
    }

    // ----------------------------------------------------- offline login
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_login_offline(move |name| {
            let name = name.trim().to_string();
            if name.is_empty() {
                return;
            }
            let uuid = ely::offline_uuid(&name);
            let account = Account {
                id: format!("offline:{name}"),
                username: name.clone(),
                uuid,
                kind: "offline".into(),
                access_token: String::new(),
                client_token: String::new(),
                skin_url: String::new(),
            };
            {
                let mut guard = state.lock().unwrap();
                guard.store.accounts.retain(|a| a.id != account.id);
                guard.store.active_account = account.id.clone();
                guard.store.accounts.push(account);
            }
            save_store(&state);
            if let Some(ui) = weak.upgrade() {
                ui.set_show_login(false);
                refresh_accounts(&ui, &state);
                log_line(&ui, "ok", &format!("Добавлен оффлайн-аккаунт: {name}"));
            }
        });
    }

    // ------------------------------------------------------- account ops
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_set_active_account(move |id| {
            state.lock().unwrap().store.active_account = id.to_string();
            save_store(&state);
            if let Some(ui) = weak.upgrade() {
                refresh_accounts(&ui, &state);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_remove_account(move |id| {
            let id = id.to_string();
            {
                let mut guard = state.lock().unwrap();
                if let Some(account) = guard.store.accounts.iter().find(|a| a.id == id).cloned() {
                    if account.is_ely() && !account.access_token.is_empty() {
                        let (token, client) = (account.access_token.clone(), account.client_token.clone());
                        std::thread::spawn(move || ely::invalidate(&token, &client));
                    }
                }
                guard.store.accounts.retain(|a| a.id != id);
                guard.heads.remove(&id);
                if guard.store.active_account == id {
                    guard.store.active_account =
                        guard.store.accounts.first().map(|a| a.id.clone()).unwrap_or_default();
                }
            }
            save_store(&state);
            if let Some(ui) = weak.upgrade() {
                refresh_accounts(&ui, &state);
            }
        });
    }

    // ---------------------------------------------------------- settings
    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_save_settings(move || {
            let Some(ui) = weak.upgrade() else { return };
            {
                let mut guard = state.lock().unwrap();
                guard.store.settings = Settings {
                    java_path: ui.get_java_path().to_string(),
                    max_memory: ui.get_max_memory().max(512) as u32,
                    jvm_args: ui.get_jvm_args().to_string(),
                    window_width: ui.get_win_width().max(320) as u32,
                    window_height: ui.get_win_height().max(240) as u32,
                    fullscreen: ui.get_fullscreen(),
                    hide_launcher: ui.get_hide_launcher(),
                    auto_java: ui.get_auto_java(),
                    russian: ui.global::<Tr>().get_ru(),
                    light_theme: ui.global::<Theme>().get_light(),
                };
            }
            save_store(&state);
            ui.set_toast(if ui.global::<Tr>().get_ru() { "Настройки сохранены".into() } else { SharedString::from("Settings saved") });
            let weak2 = ui.as_weak();
            slint::Timer::single_shot(std::time::Duration::from_secs(2), move || {
                if let Some(ui) = weak2.upgrade() {
                    ui.set_toast(SharedString::new());
                }
            });
        });
    }

    {
        let weak = ui.as_weak();
        let state = state.clone();
        ui.on_detect_java(move || {
            let weak = weak.clone();
            let state = state.clone();
            std::thread::spawn(move || {
                let configured = state.lock().unwrap().store.settings.java_path.clone();
                let found = launch::detect_java(&configured);
                let (path, text) = match &found {
                    Some(p) => (
                        p.display().to_string(),
                        format!("{} — {}", p.display(), launch::java_version_string(p)),
                    ),
                    None => (String::new(), "Java не найдена / Java not found".to_string()),
                };
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    ui.set_detected_java(text.clone().into());
                    if !path.is_empty() && ui.get_java_path().is_empty() {
                        ui.set_java_path(path.into());
                    }
                    log_line(&ui, "info", &format!("Java: {text}"));
                });
            });
        });
    }

    {
        let state = state.clone();
        ui.on_open_data_dir(move || {
            let dir = state.lock().unwrap().paths.root.clone();
            let _ = std::fs::create_dir_all(&dir);
            util::open_path(&dir.display().to_string());
        });
    }

    ui.on_open_url(|url| util::open_path(&url.to_string()));

    // --------------------------------------------------------------- log
    {
        let weak = ui.as_weak();
        ui.on_clear_log(move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_log_lines(ModelRc::new(VecModel::<LogLine>::default()));
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_copy_log(move || {
            let Some(ui) = weak.upgrade() else { return };
            let model = ui.get_log_lines();
            let text = model
                .iter()
                .map(|l| l.text.to_string())
                .collect::<Vec<_>>()
                .join("\n");
            let path = util::data_dir().join("latest-log.txt");
            match std::fs::write(&path, text) {
                Ok(()) => log_line(&ui, "ok", &format!("Журнал сохранён: {}", path.display())),
                Err(e) => log_line(&ui, "error", &format!("Не удалось сохранить журнал: {e}")),
            }
        });
    }

    ui.on_cancel_task(|| {});
}

// ------------------------------------------------------------ launch flow

fn run_instance(weak: &Weak<AppWindow>, state: &Shared, id: &str) -> anyhow::Result<()> {
    use anyhow::{anyhow, Context};

    let (instance, account, settings, root) = {
        let guard = state.lock().unwrap();
        let instance = guard
            .store
            .instance(id)
            .cloned()
            .ok_or_else(|| anyhow!("instance not found"))?;
        let account = guard
            .store
            .active()
            .cloned()
            .ok_or_else(|| anyhow!("no active account"))?;
        (instance, account, guard.store.settings.clone(), guard.paths.root.clone())
    };

    let paths = Paths::new(root);
    log_from_thread(weak, "info", format!("Запуск «{}» ({})", instance.name, instance.version));

    // -- version metadata
    let detail = {
        let weak2 = weak.clone();
        let _ = weak.upgrade_in_event_loop(|ui| set_busy(&ui, true, "Метаданные версии / Version metadata", 0.01));
        let _ = &weak2;
        install::load_version(&paths, None, &instance.version)
            .with_context(|| format!("loading version {}", instance.version))?
    };

    // -- downloads
    let reporter: Reporter = {
        let weak = weak.clone();
        Arc::new(move |text: &str, progress: f32| {
            let text = text.to_string();
            let _ = weak.upgrade_in_event_loop(move |ui| set_busy(&ui, true, &text, progress));
        })
    };
    install::install_version(&paths, &detail, &reporter).context("installing version files")?;

    // -- authlib-injector for Ely.by accounts
    let injector = if account.is_ely() {
        log_from_thread(weak, "info", "Загрузка authlib-injector...".into());
        Some(ely::ensure_authlib_injector(&paths.root).context("downloading authlib-injector")?)
    } else {
        None
    };

    // -- refresh the Ely.by token if it went stale
    let mut account = account;
    if account.is_ely() && !account.access_token.is_empty() {
        if !ely::validate(&account.access_token, &account.client_token) {
            log_from_thread(weak, "warn", "Токен устарел, обновляем... / Refreshing token...".into());
            match ely::refresh(&account.access_token, &account.client_token) {
                Ok(token) => {
                    account.access_token = token.clone();
                    let mut guard = state.lock().unwrap();
                    if let Some(stored) = guard.store.accounts.iter_mut().find(|a| a.id == account.id) {
                        stored.access_token = token;
                    }
                    let _ = guard.store.save();
                }
                Err(e) => {
                    log_from_thread(
                        weak,
                        "error",
                        format!("Не удалось обновить токен, войдите заново / Re-login required: {e:#}"),
                    );
                    let _ = weak.upgrade_in_event_loop(|ui| ui.invoke_open_login());
                    return Err(anyhow!("Ely.by session expired"));
                }
            }
        }
    }

    // -- build the command line
    let instance_dir = paths.instances().join(&instance.folder);
    std::fs::create_dir_all(&instance_dir)?;

    let spec = launch::LaunchSpec {
        paths: &paths,
        detail: &detail,
        instance_dir: instance_dir.clone(),
        account: &account,
        settings: &settings,
        authlib_injector: injector,
    };
    let cmd = launch::build_command(&spec)?;

    log_from_thread(weak, "info", format!("Java: {}", cmd.program.display()));
    log_from_thread(weak, "info", format!("Аргументы / Args: {} шт.", cmd.args.len()));
    if account.is_ely() {
        log_from_thread(weak, "ok", "authlib-injector: ely.by".into());
    }

    // -- go
    let child = {
        let weak = weak.clone();
        launch::spawn(cmd, move |line, is_err| {
            log_from_thread(&weak, if is_err { "error" } else { "game" }, line);
        })?
    };
    let pid = child.id();

    {
        let mut guard = state.lock().unwrap();
        guard.running.insert(instance.id.clone(), pid);
        if let Some(stored) = guard.store.instances.iter_mut().find(|i| i.id == instance.id) {
            stored.last_played = today();
        }
        let _ = guard.store.save();
    }

    let state2 = state.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        set_busy(&ui, false, "", 0.0);
        refresh_instances(&ui, &state2);
        if ui.get_hide_launcher() {
            let _ = ui.window().set_minimized(true);
        }
    });
    log_from_thread(weak, "ok", format!("Игра запущена (pid {pid})"));

    // -- wait for the game in a detached thread so the UI stays responsive
    let weak2 = weak.clone();
    let state3 = state.clone();
    let instance_id = instance.id.clone();
    std::thread::spawn(move || {
        let mut child = child;
        let status = child.wait();
        state3.lock().unwrap().running.remove(&instance_id);

        let text = match status {
            Ok(s) if s.success() => "Игра закрыта / Game exited normally".to_string(),
            Ok(s) => format!("Игра завершилась с кодом / Game exited with code {:?}", s.code()),
            Err(e) => format!("Ошибка ожидания процесса: {e}"),
        };
        let ok = matches!(status, Ok(s) if s.success());

        let _ = weak2.upgrade_in_event_loop(move |ui| {
            log_line(&ui, if ok { "ok" } else { "warn" }, &text);
            refresh_instances(&ui, &state3);
            let _ = ui.window().set_minimized(false);
        });
    });

    Ok(())
}
