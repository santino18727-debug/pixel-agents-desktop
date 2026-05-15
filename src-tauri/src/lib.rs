pub mod asset_loader;
pub mod error;
pub mod file_watcher;
pub mod hooks_server;
pub mod jsonl_parser;
pub mod layout_persistence;
pub mod session_map;
pub mod session_registry;
pub mod settings;

use error::MutexExt;
use file_watcher::start_watcher;
use layout_persistence::{load_layout, save_layout};
use session_registry::{list_sessions, new_registry, scan_projects};
use settings::{get_settings, set_settings};

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::Manager;
use tauri_plugin_window_state::{AppHandleExt, StateFlags, WindowExt};
use tracing::error;

/// Tauri command — opens ~/.claude/projects/ in the system file explorer.
/// Silently no-ops if the directory doesn't exist yet.
#[tauri::command]
fn open_sessions_folder() -> Result<(), String> {
    let home = dirs::home_dir().ok_or("Cannot resolve home directory")?;
    let projects_root = home.join(".claude").join("projects");

    // Create the directory if absent so the shell opener doesn't error.
    if !projects_root.exists() {
        std::fs::create_dir_all(&projects_root)
            .map_err(|e| format!("Failed to create sessions folder: {e}"))?;
    }

    // Use the opener plugin (or fall back to platform shell command).
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(projects_root.to_string_lossy().as_ref())
            .spawn()
            .map_err(|e| format!("Failed to open explorer: {e}"))?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(projects_root.to_string_lossy().as_ref())
            .spawn()
            .map_err(|e| format!("Failed to open finder: {e}"))?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(projects_root.to_string_lossy().as_ref())
            .spawn()
            .map_err(|e| format!("Failed to open file manager: {e}"))?;
    }

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let registry = new_registry();
    let session_agent_map = file_watcher::new_session_agent_map();

    tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .manage(registry.clone())
        .manage(session_agent_map.clone())
        .invoke_handler(tauri::generate_handler![
            list_sessions,
            get_settings,
            set_settings,
            save_layout,
            load_layout,
            asset_loader::scan_external_assets,
            open_sessions_folder,
        ])
        .setup(move |app| {
            // Restore saved window size and position (tauri-plugin-window-state).
            // Must be called after the plugin is registered and the window exists.
            if let Some(window) = app.get_webview_window("main") {
                if let Err(e) = window.restore_state(StateFlags::SIZE | StateFlags::POSITION) {
                    tracing::warn!("Could not restore window state: {e}");
                }
            }

            // Initial project scan
            match scan_projects() {
                Ok(sessions) => {
                    let mut reg = registry.lock_or_recover();
                    *reg = sessions;
                }
                Err(e) => error!("Initial scan failed: {e}"),
            }

            // Persist window size/position on every resize or move so state is
            // saved even when the process is killed (e.g. `tauri dev` hot-reload).
            let save_handle = app.handle().clone();
            if let Some(window) = app.get_webview_window("main") {
                window.on_window_event(move |event| {
                    if matches!(
                        event,
                        tauri::WindowEvent::Resized(_) | tauri::WindowEvent::Moved(_)
                    ) {
                        let _ = save_handle
                            .save_window_state(StateFlags::SIZE | StateFlags::POSITION);
                    }
                });
            }

            // System tray icon with Show/Quit menu.
            let show_item = MenuItem::with_id(app, "show", "Show Window", true, None::<&str>)
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
            let tray_menu = Menu::with_items(app, &[&show_item, &quit_item])
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

            TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&tray_menu)
                .tooltip("Pixel Agents Desktop")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => app.exit(0),
                    "show" => {
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        if let Some(win) = tray.app_handle().get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                })
                .build(app)
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

            // F2: Watch layout.json for external manual edits.
            layout_persistence::start_layout_watcher(app.handle().clone());

            // session_agent_map is created before setup() and managed as Tauri state
            // so list_sessions can resolve the correct agent IDs for the frontend.

            // Generate a per-session token for the hooks server.
            // Expose it as an env var so Claude Code hooks config can read it.
            let hook_token = uuid::Uuid::new_v4().to_string();
            std::env::set_var("PIXEL_AGENTS_HOOK_TOKEN", &hook_token);

            // F3: Start the Claude Code Hooks API HTTP server.
            {
                let hook_handle = app.handle().clone();
                let hook_map = std::sync::Arc::clone(&session_agent_map);
                let token = hook_token.clone();
                std::thread::spawn(move || {
                    hooks_server::start(hook_handle, hook_map, token);
                });
            }

            // Start filesystem watcher (passes shared session_agent_map).
            let handle = app.handle().clone();
            let reg = registry.clone();
            let watcher_map = std::sync::Arc::clone(&session_agent_map);
            std::thread::spawn(move || {
                if let Err(e) = start_watcher(handle, reg, watcher_map) {
                    error!("File watcher failed to start: {e}");
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Tauri application");
}
