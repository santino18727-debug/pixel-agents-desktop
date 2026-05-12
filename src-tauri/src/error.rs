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

/// Extension trait for `Mutex<T>` that recovers from a poisoned state.
/// If the mutex is poisoned (a thread panicked while holding it), the error
/// is logged with file/line info and the guard is returned anyway so the app
/// keeps running. Panics remain visible in logs without crashing the process.
pub trait MutexExt<T> {
    fn lock_or_recover(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> MutexExt<T> for std::sync::Mutex<T> {
    fn lock_or_recover(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|e| {
            tracing::error!("Mutex poisoned — recovering guard");
            e.into_inner()
        })
    }
}
