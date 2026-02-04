use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("missing api key: {0}")]
    MissingApiKey(&'static str),

    #[error("missing env var: {0}")]
    MissingEnvVar(&'static str),

    #[error("no home directory found")]
    NoHomeDirectory,

    #[error("provider error: {0}")]
    Provider(String),

    #[error("telegram error: {0}")]
    Telegram(String),
}
