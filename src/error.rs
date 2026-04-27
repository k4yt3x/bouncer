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

    #[error("llm error: {}", error_chain(.0))]
    Llm(#[from] async_openai::error::OpenAIError),

    #[error("llm returned unparseable verdict: {0}")]
    LlmVerdict(String),

    #[error("telegram error: {0}")]
    Telegram(#[from] teloxide::RequestError),

    #[error("task join error: {0}")]
    Join(#[from] tokio::task::JoinError),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Render an error and every level of its `source()` chain, joined with
/// ` — caused by: `. Standard `Display` only shows the top-level message,
/// which for `reqwest::Error` is famously uninformative (e.g. "error
/// decoding response body" hiding the actual upstream HTTP status / IO
/// error). Walking the chain surfaces whatever the underlying library
/// actually saw, no editorializing.
fn error_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut out = e.to_string();
    let mut cur = e.source();
    while let Some(src) = cur {
        out.push_str(" — caused by: ");
        out.push_str(&src.to_string());
        cur = src.source();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Inner;
    impl std::fmt::Display for Inner {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("inner cause")
        }
    }
    impl std::error::Error for Inner {}

    #[derive(Debug)]
    struct Outer(Inner);
    impl std::fmt::Display for Outer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("outer message")
        }
    }
    impl std::error::Error for Outer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    #[test]
    fn error_chain_walks_sources() {
        assert_eq!(
            error_chain(&Outer(Inner)),
            "outer message — caused by: inner cause"
        );
    }

    #[test]
    fn error_chain_handles_no_source() {
        assert_eq!(error_chain(&Inner), "inner cause");
    }
}
