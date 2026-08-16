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
use slint::{ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

slint::include_modules!();

const MAX_LOG_LINES: usize = 4000;
const TICK: Duration = Duration::from_millis(120);

/// Shared, thread-safe launcher state. Worker threads only ever touch this and
/// the message queues below; all UI updates happen on the event loop timer.
struct AppState {
    store: Store,
    root: std::path::PathBuf,
    versions: Vec<ManifestEntry>,
    running: HashMap<String, u32>,
    busy: bool,
}

/// Messages produced by worker threads and drained on the UI thread.
#[derive(Default)]
struct Outbox {
    logs: Vec<(String, String)>,
    status: Option<(bool, String, f32)>,
    toast: Option<String>,
    /// (account id, skin PNG)
    skins: Vec<(String, Vec<u8>)>,
    refresh_instances: bool,
    refresh_accounts: bool,
    open_login: bool,
    minimize: Option<bool>,
    login: Option<LoginUpdate>,
    versions: Option<VersionsUpdate>,
    java: Option<String>,
}

struct LoginUpdate {
    busy: bool,
    needs_totp: bool,
    error: String,
    close: bool,
}

struct VersionsUpdate {
    items: Vec<VersionItem>,
    loading: bool,
    error: String,
}

type Shared = Arc<Mutex<AppState>>;
type Mail = Arc<Mutex<Outbox>>;

// ------------------------------------------------------------------ helpers

fn push_log(mail: &Mail, level: &str, text: impl Into<String>) {
    mail.lock().unwrap().logs.push((level.to_string(), text.into()));
}

fn set_status(mail: &Mail, busy: bool, text: impl Into<String>, progress: f32) {
    mail.lock().unwrap().status = Some((busy, text.into(), progress));
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
        .unwrap_or(0) as i64;
    // civil_from_days (Howard Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

fn hue_for(text: &str) -> f32 {
    let mut h: u32 = 2166136261;
    for b in text.as_bytes() {
        h ^= *b as u32;
        h = h.wrapping_mul(16777619);
    }
    (h % 360) as f32
}

fn initials(name: &str) -> String {
    let letters: String = name.chars().filter(|c| c.is_alphanumeric()).take(2).collect();
    if letters.is_empty() {
        "?".to_string()
    } else {
        letters.to_uppercase()
    }
}

fn first_letter(name: &str) -> String {
    match name.chars().find(|c| c.is_alphanumeric()) {
        Some(c) => c.to_uppercase().to_string(),
        None => "?".to_string(),
    }
}

type Heads = Rc<RefCell<HashMap<String, slint::Image>>>;

fn append_log(ui: &AppWindow, level: &str, text: &str) {
    let model = ui.get_log_lines();
    let Some(vec_model) = model.as_any().downcast_ref::<VecModel<LogLine>>() else {
        return;
    };
    vec_model.push(LogLine {
        level: level.into(),
        text: format!("[{}] {}", timestamp(), text).into(),
    });
    while vec_model.row_count() > MAX_LOG_LINES {
        vec_model.remove(0);
    }
}

fn refresh_instances(ui: &AppWindow, state: &Shared) {
    let guard = state.lock().unwrap();
    let paths = Paths::new(guard.root.clone());
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
            icon_text: initials(&i.name).into(),
        })
        .collect();
    ui.set_instances(ModelRc::new(VecModel::from(items)));
}

fn refresh_accounts(ui: &AppWindow, state: &Shared, heads: &Heads) {
    let guard = state.lock().unwrap();
    let heads = heads.borrow();
    let active_id = guard.store.active_account.clone();
    let blank = slint::Image::default();

    let items: Vec<AccountItem> = guard
        .store
        .accounts
        .iter()
        .map(|a| {
            let head = heads.get(&a.id).cloned();
            AccountItem {
                id: a.id.clone().into(),
                username: a.username.clone().into(),
                uuid: a.uuid.clone().into(),
                kind: a.kind.clone().into(),
                initial: first_letter(&a.username).into(),
                active: a.id == active_id,
                has_head: head.is_some(),
                head: head.unwrap_or_else(|| blank.clone()),
            }
        })
        .collect();

    let active = guard.store.active();
    ui.set_active_account(active.map(|a| a.username.clone()).unwrap_or_default().into());
    ui.set_active_initial(active.map(|a| first_letter(&a.username)).unwrap_or_else(|| "?".into()).into());
    ui.set_active_is_ely(active.map(|a| a.is_ely()).unwrap_or(false));
    match active.and_then(|a| heads.get(&a.id).cloned()) {
        Some(head) => {
            ui.set_active_head(head);
            ui.set_active_has_head(true);
        }
        None => ui.set_active_has_head(false),
    }

    ui.set_accounts(ModelRc::new(VecModel::from(items)));
}

/// Kick off skin downloads for Ely.by accounts that don't have a head yet.
fn fetch_missing_skins(state: &Shared, mail: &Mail, heads: &Heads) {
    let known: Vec<String> = heads.borrow().keys().cloned().collect();
    let todo: Vec<(String, String)> = {
        let guard = state.lock().unwrap();
        guard
            .store
            .accounts
            .iter()
            .filter(|a| a.is_ely() && !known.contains(&a.id))
            .map(|a| (a.id.clone(), a.username.clone()))
            .collect()
    };
    if todo.is_empty() {
        return;
    }

    let mail = mail.clone();
    std::thread::spawn(move || {
        for (id, username) in todo {
            if let Ok(png) = ely::fetch_skin(&username) {
                mail.lock().unwrap().skins.push((id, png));
            }
        }
    });
}

fn save_store(state: &Shared) {
    if let Err(e) = state.lock().unwrap().store.save() {
        eprintln!("cannot save config: {e:#}");
    }
}

// --------------------------------------------------------------------- main

fn main() -> Result<(), slint::PlatformError> {
    let root = util::data_dir();
    let _ = std::fs::create_dir_all(&root);

    let state: Shared = Arc::new(Mutex::new(AppState {
        store: Store::load(),
        root: root.clone(),
        versions: Vec::new(),
        running: HashMap::new(),
        busy: false,
    }));
    let mail: Mail = Arc::new(Mutex::new(Outbox::default()));
    let heads: Heads = Rc::new(RefCell::new(HashMap::new()));

    let ui = AppWindow::new()?;
    ui.set_app_version(env!("CARGO_PKG_VERSION").into());
    ui.set_data_dir(root.display().to_string().into());
    ui.set_log_lines(ModelRc::new(VecModel::<LogLine>::default()));
    ui.set_instances(ModelRc::new(VecModel::<InstanceItem>::default()));
    ui.set_accounts(ModelRc::new(VecModel::<AccountItem>::default()));
    ui.set_versions(ModelRc::new(VecModel::<VersionItem>::default()));

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

    refresh_instances(&ui, &state);
    refresh_accounts(&ui, &state, &heads);

    append_log(&ui, "info", &format!("SlintLauncher v{}", env!("CARGO_PKG_VERSION")));
    append_log(&ui, "info", &format!("Данные / Data: {}", root.display()));

    // background Java probe
    {
        let state = state.clone();
        let mail = mail.clone();
        std::thread::spawn(move || {
            let configured = state.lock().unwrap().store.settings.java_path.clone();
            let text = match launch::detect_java(&configured) {
                Some(path) => format!("{} — {}", path.display(), launch::java_version_string(&path)),
                None => "Java не найдена / Java not found".to_string(),
            };
            let found = !text.contains("not found");
            let mut out = mail.lock().unwrap();
            out.java = Some(text.clone());
            out.logs.push((
                if found { "ok".into() } else { "warn".into() },
                format!("Java: {text}"),
            ));
        });
    }

    fetch_missing_skins(&state, &mail, &heads);
    install_callbacks(&ui, &state, &mail);

    // Single UI-thread pump: drains worker messages into the widgets.
    let pump = Timer::default();
    {
        let weak = ui.as_weak();
        let mail = mail.clone();
        let state = state.clone();
        let heads = heads.clone();
        let mut toast_ticks: i32 = 0;

        pump.start(TimerMode::Repeated, TICK, move || {
            let Some(ui) = weak.upgrade() else { return };
            let batch = std::mem::take(&mut *mail.lock().unwrap());

            for (level, text) in &batch.logs {
                append_log(&ui, level, text);
            }

            if let Some((busy, text, progress)) = batch.status {
                ui.set_busy(busy);
                ui.set_status_text(text.into());
                ui.set_progress(progress);
            }

            if let Some(text) = batch.toast {
                ui.set_toast(text.into());
                toast_ticks = 25;
            } else if toast_ticks > 0 {
                toast_ticks -= 1;
                if toast_ticks == 0 {
                    ui.set_toast(SharedString::new());
                }
            }

            let mut accounts_dirty = batch.refresh_accounts;
            for (id, png) in &batch.skins {
                if let Ok(head) = skin::head_from_skin(png) {
                    heads.borrow_mut().insert(id.clone(), head);
                    accounts_dirty = true;
                }
            }

            if batch.refresh_instances {
                refresh_instances(&ui, &state);
            }
            if accounts_dirty {
                refresh_accounts(&ui, &state, &heads);
            }
            if batch.open_login {
                ui.set_show_login(true);
            }
            if let Some(minimized) = batch.minimize {
                ui.window().set_minimized(minimized);
            }
            if let Some(java) = batch.java {
                ui.set_detected_java(java.clone().into());
                if ui.get_java_path().is_empty() {
                    if let Some((path, _)) = java.split_once(" — ") {
                        if !path.contains("not found") {
                            ui.set_java_path(path.into());
                        }
                    }
                }
            }
            if let Some(update) = batch.login {
                ui.set_login_busy(update.busy);
                ui.set_login_needs_totp(update.needs_totp);
                ui.set_login_error(update.error.into());
                if update.close {
                    ui.set_show_login(false);
                }
            }
            if let Some(update) = batch.versions {
                ui.set_versions_loading(update.loading);
                ui.set_versions_error(update.error.into());
                if !update.items.is_empty() {
                    let first = update.items[0].id.clone();
                    ui.set_versions(ModelRc::new(VecModel::from(update.items)));
                    if ui.get_new_instance_version().is_empty() {
                        ui.set_new_instance_version(first);
                    }
                } else {
                    ui.set_versions(ModelRc::new(VecModel::from(update.items)));
                }
            }
        });
    }

    ui.run()
}

// -------------------------------------------------------------- callbacks

fn install_callbacks(ui: &AppWindow, state: &Shared, mail: &Mail) {
    // ---------------------------------------------------------- versions
    {
        let state = state.clone();
        let mail = mail.clone();
        ui.on_refresh_versions(move |snapshots, old| {
            let state = state.clone();
            let mail = mail.clone();
            mail.lock().unwrap().versions = Some(VersionsUpdate {
                items: Vec::new(),
                loading: true,
                error: String::new(),
            });

            std::thread::spawn(move || {
                let cached = state.lock().unwrap().versions.clone();
                let list = if cached.is_empty() {
                    match install::fetch_manifest() {
                        Ok(manifest) => {
                            state.lock().unwrap().versions = manifest.versions.clone();
                            manifest.versions
                        }
                        Err(e) => {
                            mail.lock().unwrap().versions = Some(VersionsUpdate {
                                items: Vec::new(),
                                loading: false,
                                error: format!("Не удалось получить список версий / Cannot fetch versions: {e:#}"),
                            });
                            return;
                        }
                    }
                } else {
                    cached
                };

                let items: Vec<VersionItem> = list
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

                mail.lock().unwrap().versions = Some(VersionsUpdate {
                    items,
                    loading: false,
                    error: String::new(),
                });
            });
        });
    }

    // ---------------------------------------------------- create instance
    {
        let state = state.clone();
        let mail = mail.clone();
        ui.on_create_instance(move |name, version| {
            let (name, version) = (name.trim().to_string(), version.to_string());
            if name.is_empty() || version.is_empty() {
                return;
            }

            let id = uuid::Uuid::new_v4().to_string();
            let folder = util::sanitize_folder_name(&name);
            {
                let mut guard = state.lock().unwrap();
                let kind = guard
                    .versions
                    .iter()
                    .find(|v| v.id == version)
                    .map(|v| v.kind.clone())
                    .unwrap_or_else(|| "release".into());
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

            let mut out = mail.lock().unwrap();
            out.refresh_instances = true;
            out.logs.push(("ok".into(), format!("Создана сборка «{name}» ({version})")));
        });
    }

    // ---------------------------------------------------- delete instance
    {
        let state = state.clone();
        let mail = mail.clone();
        ui.on_delete_instance(move |id| {
            let id = id.to_string();
            let removed = {
                let mut guard = state.lock().unwrap();
                if guard.running.contains_key(&id) {
                    None
                } else if let Some(pos) = guard.store.instances.iter().position(|i| i.id == id) {
                    let inst = guard.store.instances.remove(pos);
                    let dir = Paths::new(guard.root.clone()).instances().join(&inst.folder);
                    let _ = std::fs::remove_dir_all(dir);
                    Some(inst)
                } else {
                    None
                }
            };
            save_store(&state);

            let mut out = mail.lock().unwrap();
            out.refresh_instances = true;
            match removed {
                Some(inst) => out.logs.push(("warn".into(), format!("Сборка «{}» удалена", inst.name))),
                None => out.logs.push(("error".into(), "Нельзя удалить запущенную сборку".into())),
            }
        });
    }

    // ------------------------------------------------------- open folder
    {
        let state = state.clone();
        ui.on_open_instance_folder(move |id| {
            let guard = state.lock().unwrap();
            if let Some(inst) = guard.store.instance(&id.to_string()) {
                let dir = Paths::new(guard.root.clone()).instances().join(&inst.folder);
                let _ = std::fs::create_dir_all(&dir);
                util::open_path(&dir.display().to_string());
            }
        });
    }

    // -------------------------------------------------------------- play
    {
        let state = state.clone();
        let mail = mail.clone();
        ui.on_play(move |id| {
            let id = id.to_string();
            let state = state.clone();
            let mail = mail.clone();

            {
                let mut guard = state.lock().unwrap();
                if guard.busy || guard.running.contains_key(&id) {
                    return;
                }
                if guard.store.active().is_none() {
                    drop(guard);
                    let mut out = mail.lock().unwrap();
                    out.logs.push((
                        "error".into(),
                        "Сначала войдите в аккаунт / Sign in first".into(),
                    ));
                    out.open_login = true;
                    return;
                }
                guard.busy = true;
            }

            set_status(&mail, true, "Подготовка / Preparing...", 0.0);

            std::thread::spawn(move || {
                if let Err(e) = run_instance(&state, &mail, &id) {
                    push_log(&mail, "error", format!("Ошибка запуска / Launch failed: {e:#}"));
                    mail.lock().unwrap().toast = Some("Ошибка запуска / Launch failed".into());
                }
                state.lock().unwrap().busy = false;
                set_status(&mail, false, "", 0.0);
                mail.lock().unwrap().refresh_instances = true;
            });
        });
    }

    // --------------------------------------------------------- ely login
    {
        let state = state.clone();
        let mail = mail.clone();
        ui.on_login_ely(move |login, password, totp| {
            let (login, password, totp) = (login.to_string(), password.to_string(), totp.to_string());
            let state = state.clone();
            let mail = mail.clone();

            mail.lock().unwrap().login = Some(LoginUpdate {
                busy: true,
                needs_totp: !totp.is_empty(),
                error: String::new(),
                close: false,
            });

            std::thread::spawn(move || {
                let client_token = uuid::Uuid::new_v4().to_string();
                match ely::login(&login, &password, &totp, &client_token) {
                    Ok(outcome) if outcome.needs_totp => {
                        mail.lock().unwrap().login = Some(LoginUpdate {
                            busy: false,
                            needs_totp: true,
                            error: "Введите код двухфакторной аутентификации / Enter your 2FA code".into(),
                            close: false,
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

                        let mut out = mail.lock().unwrap();
                        out.login = Some(LoginUpdate {
                            busy: false,
                            needs_totp: false,
                            error: String::new(),
                            close: true,
                        });
                        out.refresh_accounts = true;
                        out.toast = Some(format!("Привет, {username}!"));
                        out.logs.push(("ok".into(), format!("Вход выполнен: {username} (Ely.by)")));
                    }
                    Err(e) => {
                        let msg = format!("{e:#}");
                        let mut out = mail.lock().unwrap();
                        out.login = Some(LoginUpdate {
                            busy: false,
                            needs_totp: msg.contains("2FA") || msg.contains("two factor"),
                            error: msg.clone(),
                            close: false,
                        });
                        out.logs.push(("error".into(), format!("Ошибка входа: {msg}")));
                    }
                }
            });
        });
    }

    // ----------------------------------------------------- offline login
    {
        let state = state.clone();
        let mail = mail.clone();
        ui.on_login_offline(move |name| {
            let name = name.trim().to_string();
            if name.is_empty() {
                return;
            }
            let account = Account {
                id: format!("offline:{name}"),
                username: name.clone(),
                uuid: ely::offline_uuid(&name),
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

            let mut out = mail.lock().unwrap();
            out.refresh_accounts = true;
            out.login = Some(LoginUpdate {
                busy: false,
                needs_totp: false,
                error: String::new(),
                close: true,
            });
            out.logs.push(("ok".into(), format!("Добавлен оффлайн-аккаунт: {name}")));
        });
    }

    // ------------------------------------------------------- account ops
    {
        let state = state.clone();
        let mail = mail.clone();
        ui.on_set_active_account(move |id| {
            state.lock().unwrap().store.active_account = id.to_string();
            save_store(&state);
            mail.lock().unwrap().refresh_accounts = true;
        });
    }
    {
        let state = state.clone();
        let mail = mail.clone();
        ui.on_remove_account(move |id| {
            let id = id.to_string();
            {
                let mut guard = state.lock().unwrap();
                if let Some(account) = guard.store.accounts.iter().find(|a| a.id == id).cloned() {
                    if account.is_ely() && !account.access_token.is_empty() {
                        let (token, client) = (account.access_token, account.client_token);
                        std::thread::spawn(move || ely::invalidate(&token, &client));
                    }
                }
                guard.store.accounts.retain(|a| a.id != id);
                if guard.store.active_account == id {
                    guard.store.active_account =
                        guard.store.accounts.first().map(|a| a.id.clone()).unwrap_or_default();
                }
            }
            save_store(&state);
            mail.lock().unwrap().refresh_accounts = true;
        });
    }

    // ---------------------------------------------------------- settings
    {
        let weak = ui.as_weak();
        let state = state.clone();
        let mail = mail.clone();
        ui.on_save_settings(move || {
            let Some(ui) = weak.upgrade() else { return };
            let russian = ui.global::<Tr>().get_ru();
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
                    russian,
                    light_theme: ui.global::<Theme>().get_light(),
                };
            }
            save_store(&state);
            mail.lock().unwrap().toast = Some(
                if russian { "Настройки сохранены" } else { "Settings saved" }.to_string(),
            );
        });
    }

    {
        let state = state.clone();
        let mail = mail.clone();
        ui.on_detect_java(move || {
            let state = state.clone();
            let mail = mail.clone();
            std::thread::spawn(move || {
                let configured = state.lock().unwrap().store.settings.java_path.clone();
                let text = match launch::detect_java(&configured) {
                    Some(p) => format!("{} — {}", p.display(), launch::java_version_string(&p)),
                    None => "Java не найдена / Java not found".to_string(),
                };
                let mut out = mail.lock().unwrap();
                out.java = Some(text.clone());
                out.logs.push(("info".into(), format!("Java: {text}")));
            });
        });
    }

    {
        let state = state.clone();
        ui.on_open_data_dir(move || {
            let dir = state.lock().unwrap().root.clone();
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
        let mail = mail.clone();
        ui.on_copy_log(move || {
            let Some(ui) = weak.upgrade() else { return };
            let text = ui
                .get_log_lines()
                .iter()
                .map(|l| l.text.to_string())
                .collect::<Vec<_>>()
                .join("\n");
            let path = util::data_dir().join("latest-log.txt");
            let entry = match std::fs::write(&path, text) {
                Ok(()) => ("ok".to_string(), format!("Журнал сохранён: {}", path.display())),
                Err(e) => ("error".to_string(), format!("Не удалось сохранить журнал: {e}")),
            };
            mail.lock().unwrap().logs.push(entry);
        });
    }

    ui.on_cancel_task(|| {});
}

// ------------------------------------------------------------ launch flow

fn run_instance(state: &Shared, mail: &Mail, id: &str) -> anyhow::Result<()> {
    use anyhow::{anyhow, Context};

    let (instance, mut account, settings, root) = {
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
        (instance, account, guard.store.settings.clone(), guard.root.clone())
    };

    let paths = Paths::new(root);
    push_log(mail, "info", format!("Запуск «{}» ({})", instance.name, instance.version));

    // -- version metadata
    set_status(mail, true, "Метаданные версии / Version metadata", 0.01);
    let detail = install::load_version(&paths, None, &instance.version)
        .with_context(|| format!("loading version {}", instance.version))?;

    // -- downloads
    let reporter: Reporter = {
        let mail = mail.clone();
        Arc::new(move |text: &str, progress: f32| {
            mail.lock().unwrap().status = Some((true, text.to_string(), progress));
        })
    };
    install::install_version(&paths, &detail, &reporter).context("installing version files")?;

    // -- authlib-injector for Ely.by accounts
    let injector = if account.is_ely() {
        set_status(mail, true, "authlib-injector...", 0.98);
        push_log(mail, "info", "Загрузка authlib-injector / Fetching authlib-injector");
        Some(ely::ensure_authlib_injector(&paths.root).context("downloading authlib-injector")?)
    } else {
        None
    };

    // -- refresh a stale Ely.by token
    if account.is_ely() && !account.access_token.is_empty() {
        if !ely::validate(&account.access_token, &account.client_token) {
            push_log(mail, "warn", "Токен устарел, обновляем / Refreshing token...");
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
                    push_log(
                        mail,
                        "error",
                        format!("Сессия истекла, войдите заново / Session expired, sign in again: {e:#}"),
                    );
                    mail.lock().unwrap().open_login = true;
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
        instance_dir,
        account: &account,
        settings: &settings,
        authlib_injector: injector,
    };
    let cmd = launch::build_command(&spec)?;

    push_log(mail, "info", format!("Java: {}", cmd.program.display()));
    push_log(mail, "info", format!("Аргументов / Args: {}", cmd.args.len()));
    if account.is_ely() {
        push_log(mail, "ok", "authlib-injector: ely.by");
    }

    // -- go
    let child = {
        let mail = mail.clone();
        launch::spawn(cmd, move |line, is_err| {
            let level = if is_err { "error" } else { "game" };
            mail.lock().unwrap().logs.push((level.to_string(), line));
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

    {
        let mut out = mail.lock().unwrap();
        out.status = Some((false, String::new(), 0.0));
        out.refresh_instances = true;
        if settings.hide_launcher {
            out.minimize = Some(true);
        }
        out.logs.push(("ok".into(), format!("Игра запущена / Game started (pid {pid})")));
    }

    // -- reap the process without blocking the launcher
    let mail2 = mail.clone();
    let state2 = state.clone();
    let instance_id = instance.id.clone();
    std::thread::spawn(move || {
        let mut child = child;
        let status = child.wait();
        state2.lock().unwrap().running.remove(&instance_id);

        let (level, text) = match status {
            Ok(s) if s.success() => ("ok", "Игра закрыта / Game exited normally".to_string()),
            Ok(s) => (
                "warn",
                format!("Игра завершилась с кодом / Game exited with code {:?}", s.code()),
            ),
            Err(e) => ("error", format!("Ошибка ожидания процесса: {e}")),
        };

        let mut out = mail2.lock().unwrap();
        out.logs.push((level.to_string(), text));
        out.refresh_instances = true;
        out.minimize = Some(false);
    });

    Ok(())
}
