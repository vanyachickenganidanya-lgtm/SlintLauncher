# SlintLauncher

Быстрый лаунчер Minecraft на **Rust + [Slint](https://slint.dev)** с авторизацией через **[Ely.by](https://ely.by)** (в духе PrismLauncher/ElyPrismLauncher).

*A fast Minecraft launcher written in Rust + Slint with Ely.by authentication.*

![build](https://github.com/vanyachickenganidanya-lgtm/SlintLauncher/actions/workflows/build.yml/badge.svg)

---

## Возможности / Features

- 🔐 **Вход через Ely.by** — `authserver.ely.by` (Yggdrasil API), поддержка **2FA**, обновление и инвалидация токенов.
- 🧩 **authlib-injector** — скачивается автоматически и подключается как `-javaagent:authlib-injector.jar=ely.by`, поэтому скины, плащи и вход на серверы работают из коробки.
- 📦 **Установка версий с серверов Mojang** — манифест `version_manifest_v2.json`, клиентский jar, библиотеки, нативы и ассеты, всё с проверкой **SHA-1** и параллельной загрузкой.
- 🧱 **Сборки (instances)** — у каждой своя папка `instances/<name>` с сейвами, ресурспаками и модами.
- 👤 **Оффлайн-аккаунты** для одиночной игры.
- 📜 **Живой журнал** вывода игры (stdout/stderr) прямо в лаунчере.
- ⚙️ **Настройки** — путь к Java (с автопоиском), лимит памяти, аргументы JVM, размер окна/полный экран, сворачивание лаунчера.
- 🌍 **Русский и английский** интерфейс, тёмная и светлая темы.
- 🪶 Один самодостаточный бинарник, без Electron и без Qt.

## Сборка в GitHub Actions / CI

> ⚠️ **Один ручной шаг.** Workflow лежит в [`ci/build.yml`](ci/build.yml), а не в
> `.github/workflows/`, потому что токен бота не имеет права `workflows`.
> Чтобы включить автосборку, выполните один раз:
>
> ```bash
> git checkout arena/01a00a73-slintlauncher
> mkdir -p .github/workflows && git mv ci/build.yml .github/workflows/build.yml
> git commit -m "Enable CI" && git push
> ```

После этого сборки для **Windows** и **Linux** появятся в
**Actions → Build** → последний запуск → раздел *Artifacts*.

| Платформа | Артефакт |
|---|---|
| Windows x86_64 | `slint-launcher-windows-x86_64` |
| Linux x86_64 | `slint-launcher-linux-x86_64` |

## Как это работает / How it works

```
Вход Ely.by  ──►  POST authserver.ely.by/auth/authenticate
                  { username, password[:2FA], clientToken, requestUser }
                        │
                        ▼  accessToken + selectedProfile
Запуск игры  ──►  java -javaagent:authlib-injector.jar=ely.by \
                       -Xmx<N>M -cp <libs+client.jar> <mainClass> \
                       --username <ник> --uuid <uuid> --accessToken <token> ...
```

authlib-injector перехватывает обращения клиента к серверам сессий Mojang
и перенаправляет их на Ely.by — правки самого клиента не нужны.

## Сборка из исходников / Building

Требуется Rust 1.77+.

```bash
git clone https://github.com/vanyachickenganidanya-lgtm/SlintLauncher
cd SlintLauncher
cargo build --release
./target/release/slint-launcher
```

### Проверка интерфейса без Rust

Компилятор Slint доступен и в виде Python-пакета, поэтому синтаксис UI и
соответствие имён между Rust и Slint можно проверить без сборки всего проекта:

```bash
pip install slint
python3 tools/check_ui.py
```

### Зависимости Linux

```bash
sudo apt install libfontconfig-dev libxcb1-dev libxcb-render0-dev \
  libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev \
  libxkbcommon-x11-dev libwayland-dev libinput-dev libudev-dev
```

Для запуска игры нужна установленная **Java** (17+ для 1.17+, 8 для старых версий).

## Где лежат файлы / Data location

| ОС | Путь |
|---|---|
| Windows | `%APPDATA%\SlintLauncher` |
| Linux | `~/.local/share/SlintLauncher` |
| macOS | `~/Library/Application Support/SlintLauncher` |

Переопределяется переменной окружения `SLINTLAUNCHER_HOME`.

```
SlintLauncher/
├── launcher.json        # настройки, сборки, аккаунты
├── versions/<id>/       # <id>.json, <id>.jar, natives/
├── libraries/           # библиотеки + authlib-injector
├── assets/              # indexes/, objects/
└── instances/<name>/    # игровые папки (saves, mods, ...)
```

## Структура проекта

```
ui/                 интерфейс на Slint
├── app.slint       главное окно, навигация, оверлеи
├── theme.slint     палитра и токены
├── strings.slint   строки RU/EN
├── widgets.slint   кнопки, поля, переключатели
├── dialogs.slint   вход Ely.by, создание сборки
└── pages/          страницы: сборки, аккаунты, настройки, журнал, о программе

src/
├── main.rs             склейка UI и логики
├── config.rs           launcher.json
├── ely.rs              Ely.by API + authlib-injector
├── skin.rs             рендер головы из скина
├── util.rs             загрузка, SHA-1, пути
└── minecraft/
    ├── manifest.rs     типы версий Mojang, правила OS
    ├── install.rs      загрузка jar/библиотек/ассетов, распаковка нативов
    └── launch.rs       поиск Java, аргументы, запуск процесса
```

## Безопасность / Security

Пароль Ely.by **никуда не сохраняется** — хранится только `accessToken`/`clientToken`
в `launcher.json`. Кнопка «Выйти» отзывает токен через `/auth/invalidate`.

## Лицензия

GPL-3.0. Проект не связан с Mojang, Microsoft или Ely.by.
