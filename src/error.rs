use thiserror::Error;

/// redact secrets from URLs in error messages.
/// replaces telegram bot tokens (bot<id>:<token>) with bot[REDACTED].
fn redact_url(err: &reqwest::Error) -> String {
    let msg = err.to_string();
    // telegram bot token pattern: bot<digits>:<alphanumeric>/<method>
    if let Some(start) = msg.find("bot")
        && let Some(slash) = msg[start..].find('/')
    {
        let token_part = &msg[start..start + slash];
        if token_part.contains(':') {
            return msg.replace(token_part, "bot[REDACTED]");
        }
    }
    msg
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("http error: {}", redact_url(.0))]
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

    #[error("mcp error: {0}")]
    Mcp(String),
}
