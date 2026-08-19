use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Message(String),
    #[error("сеть: {0}")]
    Http(String),
    #[error("не удалось разобрать ответ API: {0}")]
    Json(#[from] serde_json::Error),
    #[error("ввод/вывод {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("java не найдена — укажите путь в настройках")]
    JavaMissing,
    #[error("версия {0} не найдена в манифесте Mojang")]
    VersionNotFound(String),
    #[error("хеш не совпал: {path}")]
    Checksum { path: PathBuf },
    #[error("нет активного аккаунта")]
    NoAccount,
    #[error("вход Microsoft: {0}")]
    Auth(String),
}

impl Error {
    pub fn msg(text: impl Into<String>) -> Self {
        Self::Message(text.into())
    }
}

impl From<reqwest::Error> for Error {
    fn from(value: reqwest::Error) -> Self {
        Self::Http(value.without_url().to_string())
    }
}

pub fn io_err(path: impl Into<PathBuf>, source: std::io::Error) -> Error {
    Error::Io {
        path: path.into(),
        source,
    }
}
