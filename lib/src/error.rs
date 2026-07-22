#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{provider}: authentication rejected — check the credential/scope")]
    Auth { provider: &'static str },

    #[error("{provider}: {resource} {id} not found")]
    NotFound {
        provider: &'static str,
        resource: &'static str,
        id: String,
    },

    #[error("{provider} API error ({status}): {message}")]
    Api {
        provider: &'static str,
        status: u16,
        message: String,
    },

    #[error("request failed: {0}")]
    Transport(#[from] reqwest::Error),

    #[error("{0}")]
    InvalidRequest(String),
}

pub type Result<T> = std::result::Result<T, Error>;
