use thiserror::Error;

#[derive(Error, Debug, PartialEq)]
pub enum HanzoEmbeddingError {
    #[error("Request failed")]
    RequestFailed(String),
    #[error("Invalid model architecture")]
    InvalidModelArchitecture,
    #[error("Unimplemented model dimensions")]
    UnimplementedModelDimensions(String),
    #[error("Failed embedding generation")]
    FailedEmbeddingGeneration(String),
}

impl From<reqwest::Error> for HanzoEmbeddingError {
    fn from(error: reqwest::Error) -> Self {
        HanzoEmbeddingError::RequestFailed(error.to_string())
    }
}
