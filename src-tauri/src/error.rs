use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Watcher: {0}")]
    Watcher(#[from] notify::Error),
    #[error("Settings: {0}")]
    Settings(String),
}

impl From<AppError> for String {
    fn from(e: AppError) -> Self {
        e.to_string()
    }
}

pub type Result<T> = std::result::Result<T, AppError>;
