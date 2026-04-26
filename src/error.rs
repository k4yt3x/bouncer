use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to read config file {path}: {source}")]
    ConfigRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse config file {path}: {source}")]
    ConfigParse {
        path: PathBuf,
        #[source]
        source: serde_yaml_ng::Error,
    },

    #[error("invalid config: {0}")]
    ConfigInvalid(String),

    #[error("failed to parse embedded locale `{name}`: {source}")]
    LocaleParse {
        name: String,
        #[source]
        source: toml::de::Error,
    },

    #[error("unknown locale `{0}`")]
    UnknownLocale(String),

    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("database migration error: {0}")]
    Migration(#[from] rusqlite_migration::Error),

    #[error("llm error: {0}")]
    Llm(#[from] async_openai::error::OpenAIError),

    #[error("llm returned unparseable verdict: {0}")]
    LlmVerdict(String),

    #[error("telegram error: {0}")]
    Telegram(#[from] teloxide::RequestError),

    #[error("task join error: {0}")]
    Join(#[from] tokio::task::JoinError),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
