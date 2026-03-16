use thiserror::Error;

#[allow(dead_code)]
#[derive(Debug, Error)]
pub enum NanoDnsError {
    #[error("Config error: {0}")]
    Config(String),

    #[error("DNS parse error: {0}")]
    DnsParse(#[from] hickory_proto::error::ProtoError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("Upstream DNS error: {0}")]
    Upstream(String),

    #[error("Peer sync error: {0}")]
    Sync(String),
}

#[allow(dead_code)]
pub type Result<T> = std::result::Result<T, NanoDnsError>;
