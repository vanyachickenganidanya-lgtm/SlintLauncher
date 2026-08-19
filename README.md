# SlintLauncher

[![CI](https://github.com/vanyachickenganidanya-lgtm/SlintLauncher/actions/workflows/ci.yml/badge.svg)](https://github.com/vanyachickenganidanya-lgtm/SlintLauncher/actions/workflows/ci.yml)

Быстрый нативный лаунчер Minecraft на **Rust + Slint**.

Тёмная оболочка, изолированные инстансы, ваниль и Fabric, офлайн- и Microsoft-аккаунты, параллельная загрузка клиента, библиотек и ассетов с проверкой SHA-1.

## Возможности

- Каталог версий с Mojang Piston (`release` / `snapshot` / старые)
- Инстансы с отдельными `saves`, `mods`, `resourcepacks`, `shaderpacks`
- Общий кэш `versions` / `libraries` / `assets`
- Fabric через meta.fabricmc.net
- Офлайн-ник (UUID как у Java `OfflinePlayer:`)
- Microsoft device-code + Xbox Live + Minecraft Services
- RAM, Java, JVM-флаги, разрешение, fullscreen
- Патчноуты Mojang
- Прогресс установки и запуск через системную Java

## Сборка

Нужны Rust 1.74+, пакеты для Slint и Java 17+ (для 1.21 — лучше 21).

```bash
# Debian / Ubuntu
sudo apt install build-essential pkg-config libfontconfig1-dev \
    libxkbcommon-dev libxcb-shape0-dev libxcb-xfixes0-dev \
    libssl-dev cmake

# macOS
xcode-select --install
brew install cmake pkg-config

cargo run --release
```

Бинарник: `target/release/slintlauncher`.

Репозиторий — ещё и GitHub Action (`action.yml`): ставит зависимости Slint, гоняет `cargo test --lib` и собирает бинарник.

```yaml
- uses: actions/checkout@v4
- uses: vanyachickenganidanya-lgtm/SlintLauncher@arena/01a0187f-slintlauncher
```

Готовые workflow лежат в [`packaging/github-actions/`](packaging/github-actions). Скопируйте их в `.github/workflows/`, если у токена есть право `workflows` — тогда CI сам соберёт Linux / Windows / macOS и выложит архивы в **Actions → Artifacts**. Тег `v*` публикует релиз.

## Как играть

1. Запустите лаунчер.
2. В **Аккаунты** задайте ник или войдите через Microsoft.
3. В **Версии** выберите релиз или оставьте `latest-release`.
4. При желании создайте Fabric-инстанс.
5. Нажмите **Играть** — лаунчер докачает файлы и поднимет игру.

Данные лежат в:

| ОС | Путь |
| --- | --- |
| Linux | `~/.local/share/SlintLauncher` |
| macOS | `~/Library/Application Support/SlintLauncher` |
| Windows | `%APPDATA%\SlintLauncher` |

## Microsoft

Используется публичный Azure client id. Свой можно задать так:

```bash
export SLINTLAUNCHER_MSA_CLIENT_ID="xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
```

Приложению нужны `XboxLive.signin` и `offline_access`, плюс public client / device code.

Офлайн-аккаунт **не** пускает на лицензионные online-mode серверы.

## Архитектура

```
ui/                 Slint: тема, виджеты, окно
src/mojang.rs       манифест, version JSON, inheritFrom
src/install.rs      клиент, libraries, assets, natives
src/launch.rs       JVM/game args, classpath, spawn
src/fabric.rs       профиль Fabric Loader
src/auth.rs         Microsoft → Xbox → Minecraft
src/config.rs       аккаунты, инстансы, настройки
```

Лицензия: [AGPL-3.0-only](LICENSE). Minecraft принадлежит Mojang/Microsoft; это независимый клиент.
