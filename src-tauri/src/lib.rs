pub mod error;
pub mod file_watcher;
pub mod jsonl_parser;
pub mod session_registry;
pub mod settings;

use file_watcher::start_watcher;
use session_registry::{list_sessions, new_registry, scan_projects};
use settings::{get_settings, set_settings};

use tracing::error;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let registry = new_registry();

    tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::default().build())
        .manage(registry.clone())
        .invoke_handler(tauri::generate_handler![
            list_sessions,
            get_settings,
            set_settings,
        ])
        .setup(move |app| {
            // Initial project scan
            match scan_projects() {
                Ok(sessions) => {
                    let mut reg = registry.lock().unwrap_or_else(|e| e.into_inner());
                    *reg = sessions;
                }
                Err(e) => error!("Initial scan failed: {e}"),
            }

            // Start filesystem watcher
            let handle = app.handle().clone();
            let reg = registry.clone();
            std::thread::spawn(move || {
                if let Err(e) = start_watcher(handle, reg) {
                    error!("File watcher failed to start: {e}");
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Tauri application");
}
