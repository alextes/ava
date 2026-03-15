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

    #[error("provider error: {0}")]
    Provider(String),

    #[error("budget exhausted: {0}")]
    BudgetExhausted(String),

    #[error("rate limited: {0}")]
    RateLimited(String),

    #[error("telegram error: {0}")]
    Telegram(String),

    #[allow(dead_code)]
    #[error("command timed out after {0}s")]
    ExecTimeout(u64),

    #[allow(dead_code)]
    #[error("command denied by user")]
    ExecDenied,

    #[error("approval timed out")]
    ApprovalTimeout,

    #[error("context overflow")]
    ContextOverflow,

    #[allow(dead_code)]
    #[error("mcp error: {0}")]
    Mcp(String),
}
